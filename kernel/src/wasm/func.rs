use crate::util::zip_eq::IteratorExt;
use crate::wasm::indices::VMSharedTypeIndex;
use crate::wasm::runtime::{StaticVMOffsets, VMContext, VMFuncRef, VMFunctionImport, VMVal};
use crate::wasm::store::Stored;
use crate::wasm::translate::{WasmFuncType, WasmHeapType, WasmRefType, WasmValType};
use crate::wasm::type_registry::RegisteredType;
use crate::wasm::values::Val;
use crate::wasm::{Error, MAX_WASM_STACK, Store, Trap, runtime};
use crate::{arch, ensure, wasm};
use core::arch::asm;
use core::ffi::c_void;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::{mem, ptr};
use fallible_iterator::FallibleIterator;
use wasmparser::ValType;

/// A WebAssembly function.
#[derive(Debug, Clone, Copy)]
pub struct Func(Stored<runtime::ExportedFunction>);

impl Func {
    /// Returns the type of this function.
    ///
    /// # Panics
    ///
    /// TODO
    pub fn ty(self, store: &Store) -> FuncType {
        // Safety: at this point `VMContext` is initialized, so accessing its fields is safe
        let func_ref = unsafe { store[self.0].func_ref.as_ref() };
        let ty = store
            .engine
            .type_registry()
            .get_type(&store.engine, func_ref.type_index)
            .unwrap();
        FuncType(ty)
    }

    pub async fn call(
        &self,
        store: &mut Store,
        params: &[Val],
        results: &mut [Val],
    ) -> wasm::Result<()> {
        let ty = self.ty(store);
        let ty = ty.as_wasm_func_type();

        let mut params_ = ty.params.iter().zip_eq(params);
        while let Some((expected, param)) = params_.next().map_err(|_| Error::MismatchedTypes)? {
            let found = param.ty();
            ensure!(
                *expected == found,
                Error::MismatchedTypes,
                "Type mismatch. Expected `{expected:?}`, but found `{found:?}`"
            );
        }

        ensure!(
            results.len() >= ty.results.len(),
            Error::MismatchedTypes,
            "Results slice too small. Need space for at least {}, but got only {}",
            ty.results.len(),
            results.len()
        );

        store
            .on_fiber(|store| {
                // Safety: we have checked the
                unsafe { self.call_unchecked(store, params, results) }
            })
            .await?
    }

    /// Calls the given function with the provided arguments and places the results in the provided
    /// results slice.
    ///
    /// # Errors
    ///
    /// TODO
    ///
    /// # Safety
    ///
    /// It is up to the caller to ensure the provided arguments are of the correct types and that
    /// the `results` slice has enough space to hold the results of the function.
    unsafe fn call_unchecked(
        &self,
        store: &mut Store,
        params: &[Val],
        results: &mut [Val],
    ) -> wasm::Result<()> {
        let values_vec_size = params.len().max(results.len());

        // take out the argument storage from the store
        let mut values_vec = store.take_wasm_vmval_storage();
        debug_assert!(values_vec.is_empty());

        // copy the arguments into the storage
        values_vec.resize_with(values_vec_size, || VMVal::v128(0));
        for (arg, slot) in params.iter().copied().zip(&mut values_vec) {
            *slot = arg.as_vmval(store);
        }

        // do the actual call
        // Safety: caller has to ensure safety
        unsafe {
            self.call_unchecked_raw(store, values_vec.as_mut_ptr(), values_vec_size)?;
        }

        // copy the results out of the storage
        let ty = self.ty(store);
        let ty = ty.as_wasm_func_type();

        for ((i, slot), vmval) in results.iter_mut().enumerate().zip(&values_vec) {
            let ty = &ty.results[i];
            // Safety: caller has to ensure safety
            *slot = unsafe { Val::from_vmval(store, *vmval, ty) };
        }

        // clean up and return the argument storage
        values_vec.truncate(0);
        store.return_wasm_vmval_storage(values_vec);

        Ok(())
    }

    #[allow(
        unreachable_code,
        clippy::unnecessary_wraps,
        reason = "TODO rework in progress. see #298"
    )]
    unsafe fn call_unchecked_raw(
        &self,
        store: &mut Store,
        args_results_ptr: *mut VMVal,
        args_results_len: usize,
    ) -> wasm::Result<()> {
        // Safety: funcref is always initialized
        let func_ref = unsafe { store[self.0].func_ref.as_ref() };
        // Safety: funcref is always initialized
        let vmctx = unsafe { VMContext::from_opaque(func_ref.vmctx) };
        let module = store[store.get_instance_from_vmctx(vmctx)].module();

        let _guard = enter_wasm(vmctx, &module.offsets().static_);

        let res = wasm::trap_handler::catch_traps(vmctx, module.offsets().static_.clone(), || {
            tracing::debug!("jumping to WASM");

            // Safety: TODO
            unsafe { func_ref.array_call(vmctx, vmctx, args_results_ptr, args_results_len) }
        });

        tracing::trace!("returned from WASM {res:?}");
        match res {
            Ok(_)
            // The userspace ABI uses the Trap::Exit code to signal a graceful exit
            | Err(Error::Trap {
                      trap: Trap::Exit, ..
                  }) => Ok(()),
            Err(err) => Err(err),
        }
    }

    pub(super) fn as_raw(&self, store: &mut Store) -> *mut c_void {
        store[self.0].func_ref.as_ptr().cast()
    }

    fn as_vm_func_ref(self, store: &Store) -> NonNull<VMFuncRef> {
        store[self.0].func_ref
    }

    pub(super) fn as_vmfunction_import(&self, store: &Store) -> VMFunctionImport {
        // Safety: at this point `VMContext` is initialized, so accessing its fields is safe
        let func_ref = unsafe { store[self.0].func_ref.as_ref() };
        VMFunctionImport {
            wasm_call: func_ref.wasm_call,
            array_call: func_ref.array_call,
            vmctx: func_ref.vmctx,
        }
    }

    /// # Safety
    ///
    /// The caller must ensure `export` is a valid exported function within `store`.
    pub(super) unsafe fn from_vm_export(
        store: &mut Store,
        export: runtime::ExportedFunction,
    ) -> Self {
        Self(store.push_function(export))
    }

    /// # Safety
    ///
    /// The caller must ensure the func ref is a valid function reference in this store.
    unsafe fn from_vm_func_ref(store: &mut Store, func_ref: NonNull<VMFuncRef>) -> Self {
        // Safety: ensured by caller
        unsafe {
            debug_assert!(func_ref.as_ref().type_index != VMSharedTypeIndex::default());
            let export = runtime::ExportedFunction { func_ref };
            Self::from_vm_export(store, export)
        }
    }

    pub fn typed<Params, Results>(self, store: &Store) -> wasm::Result<TypedFunc<Params, Results>>
    where
        Params: WasmParams,
        Results: WasmResults,
    {
        let ty = self.ty(store);
        let wasm_ty = ty.as_wasm_func_type();
        Params::typecheck(wasm_ty.params.iter())?;
        Results::typecheck(wasm_ty.results.iter())?;

        Ok(TypedFunc {
            ty,
            func: self,
            _m: PhantomData,
        })
    }
}

/// A WebAssembly function type.
///
/// This is essentially a reference counted index into the engine's type registry.
pub struct FuncType(RegisteredType);

impl FuncType {
    pub(crate) fn type_index(&self) -> VMSharedTypeIndex {
        self.0.index()
    }

    pub fn as_wasm_func_type(&self) -> &WasmFuncType {
        self.0.unwrap_func()
    }

    pub(crate) fn into_registered_type(self) -> RegisteredType {
        self.0
    }

    pub fn param(&self, i: usize) -> Option<&WasmValType> {
        let func = self.0.unwrap_func();
        func.params.get(i)
    }
}

/// A WebAssembly function.
pub struct TypedFunc<Params, Results> {
    func: Func,
    ty: FuncType,
    _m: PhantomData<fn(Params) -> Results>,
}

impl<Params, Results> TypedFunc<Params, Results>
where
    Params: WasmParams,
    Results: WasmResults,
{
    /// Invokes this WebAssembly function with the specified parameters.
    ///
    /// # Errors
    ///
    /// For more information on errors see the documentation on [`Func::call`].
    pub async fn call(&self, store: &mut Store, params: Params) -> wasm::Result<Results> {
        #[cfg(debug_assertions)]
        Self::assert_typecheck(self.ty.as_wasm_func_type());

        store
            .on_fiber(|store| {
                // Safety: The `Func::typed` constructor ensured that this types generics and the
                // types required by the WASM function match
                unsafe { Self::call_raw(store, &self.ty, store[self.func.0].func_ref, params) }
            })
            .await?
    }

    /// Do the raw call of a typed function.
    ///
    /// Calling typed functions is a bit more optimized than calling untyped ones, in particular type
    /// checking has already been performed when constructing the typed function reference.
    ///
    /// # Implementation
    ///
    /// This function uses the type information encoded in the `Params` and `Results` generics to
    /// allocate the `VMVal` storage array on the stack. It then lowers the Rust parameters into their
    /// WASM `VMVal` representation and writes them to the stack allocated array.
    ///
    /// We then call into WASM through the array-call trampoline just like [`Func::call_unchecked_raw`].
    ///
    /// Once we return from WASM, we load the expected result values from the stack allocated array,
    /// convert them into their Rust representations and return.
    ///
    /// # Safety
    ///
    /// The caller must ensure the provided [`VMFuncRef`] is of the correct type for this [`TypedFunc`].
    unsafe fn call_raw(
        store: &mut Store,
        ty: &FuncType,
        func_ref: NonNull<VMFuncRef>,
        params: Params,
    ) -> wasm::Result<Results> {
        // Safety: ensured by caller
        unsafe {
            union Storage<T: Copy, U: Copy> {
                params: MaybeUninit<T>,
                results: U,
            }

            let mut storage = Storage::<Params::VMValStorage, Results::VMValStorage> {
                params: MaybeUninit::uninit(),
            };

            // Lower the Rust types into their WASM VMVal representation and store them into the buffer
            params.store(store, ty, &mut storage.params)?;

            let vmctx = VMContext::from_opaque(func_ref.as_ref().vmctx);
            let module = store[store.get_instance_from_vmctx(vmctx)].module();

            let _guard = enter_wasm(vmctx, &module.offsets().static_);

            let res =
                wasm::trap_handler::catch_traps(vmctx, module.offsets().static_.clone(), || {
                    let storage_len = size_of_val::<Storage<_, _>>(&storage) / size_of::<VMVal>();
                    let storage: *mut Storage<_, _> = &mut storage;
                    let storage = storage.cast::<VMVal>();

                    tracing::debug!("jumping to WASM");
                    func_ref
                        .as_ref()
                        .array_call(vmctx, vmctx, storage, storage_len);
                });

            tracing::trace!("returned from WASM {res:?}");

            match res {
                Ok(_)
                // The userspace ABI uses the Trap::Exit code to signal a graceful exit
                | Err(Error::Trap {
                          trap: Trap::Exit, ..
                      }) => Ok(Results::load(store, &storage.results)),
                Err(err) => Err(err),
            }
        }
    }

    fn assert_typecheck(ty: &WasmFuncType) {
        Params::typecheck(ty.params.iter()).expect("params should match");
        Results::typecheck(ty.results.iter()).expect("results should match");
    }
}

#[derive(Copy, Clone)]
pub enum TypeCheckPosition {
    Param,
    Result,
}

/// A type that can be used as an argument or return value for WASM functions.
///
/// # Safety
///
/// This trait should not be implemented manually.
pub unsafe trait WasmTy: Send {
    fn valtype() -> WasmValType;
    fn store(self, store: &mut Store, ptr: &mut MaybeUninit<VMVal>) -> wasm::Result<()>;
    unsafe fn load(store: &mut Store, ptr: &VMVal) -> Self;
    #[inline]
    fn typecheck(actual: &WasmValType, position: TypeCheckPosition) -> wasm::Result<()> {
        let expected = Self::valtype();

        match position {
            TypeCheckPosition::Result => actual.ensure_matches(&expected),
            TypeCheckPosition::Param => match (expected.get_ref(), actual.get_ref()) {
                (Some(expected_ref), Some(actual_ref)) if actual_ref.heap_type.is_concrete() => {
                    expected_ref
                        .heap_type
                        .top()
                        .ensure_matches(&actual_ref.heap_type.top())
                }
                _ => actual.ensure_matches(&expected),
            },
        }
    }
    fn dynamic_concrete_type_check(
        &self,
        store: &Store,
        nullable: bool,
        actual: &WasmHeapType,
    ) -> wasm::Result<()>;
    fn compatible_with_store(&self, store: &Store) -> bool;
}

/// A type that can be used as an argument for WASM functions.
///
/// This trait is implemented for bare types that may be passed to WASM and tuples of those types.
///
/// # Safety
///
/// This trait should not be implemented manually.
pub unsafe trait WasmParams: Send {
    type VMValStorage: Copy;
    fn typecheck<'a>(params: impl ExactSizeIterator<Item = &'a WasmValType>) -> wasm::Result<()>;
    fn store(
        self,
        store: &mut Store,
        func_ty: &FuncType,
        dst: &mut MaybeUninit<Self::VMValStorage>,
    ) -> wasm::Result<()>;
}

/// A type that may be returned from WASM functions.
///
/// This trait is implemented for bare types that may be passed to WASM and tuples of those types.
///
/// # Safety
///
/// This trait should not be implemented manually.
pub unsafe trait WasmResults: WasmParams {
    unsafe fn load(store: &mut Store, abi: &Self::VMValStorage) -> Self;
}

macro_rules! integers {
    ($($integer:ident/$get_integer:ident => $ty:ident)*) => ($(
        // Safety: this macro correctly delegates to the integer methods
        unsafe impl WasmTy for $integer {
            #[inline]
            fn valtype() -> WasmValType {
                WasmValType::$ty
            }

            #[inline]
            fn store(self, _store: &mut Store, ptr: &mut MaybeUninit<VMVal>) -> wasm::Result<()> {
                ptr.write(VMVal::$integer(self));
                Ok(())
            }

            #[inline]
            unsafe fn load(_store: &mut Store, ptr: &VMVal) -> Self {
                ptr.$get_integer()
            }

            #[inline]
            fn dynamic_concrete_type_check(
                &self,
                _store: &Store,
                _nullable: bool,
                _actual: &WasmHeapType,
            ) -> wasm::Result<()> {
                unreachable!("`dynamic_concrete_type_check` not implemented for integers");
            }

            #[inline]
            fn compatible_with_store(&self, _store: &Store) -> bool {
                true
            }
        }
    )*)
}

integers! {
    i32/get_i32 => I32
    i64/get_i64 => I64
    u32/get_u32 => I32
    u64/get_u64 => I64
}

macro_rules! floats {
    ($($float:ident/$get_float:ident => $ty:ident)*) => ($(
        // Safety: this macro correctly delegates to the float methods
        unsafe impl WasmTy for $float {
            #[inline]
            fn valtype() -> WasmValType {
                WasmValType::$ty
            }

            #[inline]
            fn store(self, _store: &mut Store, ptr: &mut MaybeUninit<VMVal>) -> wasm::Result<()> {
                ptr.write(VMVal::$float(self.to_bits()));
                Ok(())
            }

            #[inline]
            unsafe fn load(_store: &mut Store, ptr: &VMVal) -> Self {
                $float::from_bits(ptr.$get_float())
            }

            #[inline]
            fn dynamic_concrete_type_check(
                &self,
                _store: &Store,
                _nullable: bool,
                _actual: &WasmHeapType,
            ) -> wasm::Result<()> {
                unreachable!("`dynamic_concrete_type_check` not implemented for floats");
            }

            #[inline]
            fn compatible_with_store(&self, _store: &Store) -> bool {
                true
            }
        }
    )*)
}

floats! {
    f32/get_f32 => F32
    f64/get_f64 => F64
}

// Safety: functions are lowered as VMFuncRef pointers. TODO the correctness of this should be checked by tests
unsafe impl WasmTy for Func {
    fn valtype() -> WasmValType {
        WasmValType::Ref(WasmRefType::FUNCREF)
    }

    fn store(self, store: &mut Store, ptr: &mut MaybeUninit<VMVal>) -> wasm::Result<()> {
        let raw = self.as_vm_func_ref(store).as_ptr();
        ptr.write(VMVal::funcref(raw.cast::<c_void>()));
        Ok(())
    }

    unsafe fn load(store: &mut Store, ptr: &VMVal) -> Self {
        // Safety: ensured by caller
        unsafe {
            let ptr = NonNull::new(ptr.get_funcref()).unwrap().cast();
            Func::from_vm_func_ref(store, ptr)
        }
    }

    fn dynamic_concrete_type_check(
        &self,
        _store: &Store,
        _nullable: bool,
        _actual: &WasmHeapType,
    ) -> wasm::Result<()> {
        todo!()
    }

    fn compatible_with_store(&self, store: &Store) -> bool {
        store.has_function(self.0)
    }
}

// Safety: functions are lowered as VMFuncRef pointers. TODO the correctness of this should be checked by tests
unsafe impl WasmTy for Option<Func> {
    fn valtype() -> WasmValType {
        WasmValType::Ref(WasmRefType::FUNCREF)
    }

    fn store(self, store: &mut Store, ptr: &mut MaybeUninit<VMVal>) -> wasm::Result<()> {
        let raw = if let Some(f) = self {
            f.as_vm_func_ref(store).as_ptr()
        } else {
            ptr::null_mut()
        };
        ptr.write(VMVal::funcref(raw.cast::<c_void>()));
        Ok(())
    }

    unsafe fn load(store: &mut Store, ptr: &VMVal) -> Self {
        // Safety: ensured by caller
        unsafe {
            let ptr = NonNull::new(ptr.get_funcref())?.cast();
            Some(Func::from_vm_func_ref(store, ptr))
        }
    }

    fn dynamic_concrete_type_check(
        &self,
        _store: &Store,
        _nullable: bool,
        _actual: &WasmHeapType,
    ) -> wasm::Result<()> {
        todo!()
    }

    fn compatible_with_store(&self, store: &Store) -> bool {
        if let Some(f) = self {
            store.has_function(f.0)
        } else {
            true
        }
    }
}

macro_rules! for_each_function_signature {
    ($mac:ident) => {
        $mac!(0);
        $mac!(1 A1);
        $mac!(2 A1 A2);
        $mac!(3 A1 A2 A3);
        $mac!(4 A1 A2 A3 A4);
        $mac!(5 A1 A2 A3 A4 A5);
        $mac!(6 A1 A2 A3 A4 A5 A6);
        $mac!(7 A1 A2 A3 A4 A5 A6 A7);
        $mac!(8 A1 A2 A3 A4 A5 A6 A7 A8);
        $mac!(9 A1 A2 A3 A4 A5 A6 A7 A8 A9);
        $mac!(10 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10);
        $mac!(11 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11);
        $mac!(12 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 A12);
        $mac!(13 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 A12 A13);
        $mac!(14 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 A12 A13 A14);
        $mac!(15 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 A12 A13 A14 A15);
        $mac!(16 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 A12 A13 A14 A15 A16);
        $mac!(17 A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 A12 A13 A14 A15 A16 A17);
    };
}

macro_rules! impl_wasm_params {
    ($n:tt $($t:ident)*) => {
        #[allow(non_snake_case, reason = "argument names above are uppercase")]
        // Safety: see `WasmTy` for details
        unsafe impl<$($t: WasmTy,)*> WasmParams for ($($t,)*) {
            type VMValStorage = [VMVal; $n];

            fn typecheck<'a>(mut params: impl ExactSizeIterator<Item = &'a WasmValType>) -> wasm::Result<()> {
                let mut _n = 0;

                $(
                    match params.next() {
                        Some(t) => {
                            _n += 1;
                            $t::typecheck(t, TypeCheckPosition::Param)?
                        },
                        None => {
                            $crate::bail!(Error::MismatchedTypes, "expected {} types, found {}", $n as usize, params.len() + _n);
                        },
                    }
                )*

                match params.next() {
                    None => Ok(()),
                    Some(_) => {
                        _n += 1;
                        $crate::bail!(Error::MismatchedTypes, "expected {} types, found {}", $n, params.len() + _n);
                    },
                }
            }

            fn store(self, _store: &mut Store, _func_ty: &FuncType, _dst: &mut MaybeUninit<Self::VMValStorage>) -> wasm::Result<()> {
                use $crate::util::maybe_uninit::MaybeUninitExt;

                let ($($t,)*) = self;
                let mut _i: usize = 0;

                $(
                    if !$t.compatible_with_store(_store) {
                        $crate::bail!(Error::CrossStore, "attempt to pass cross-`Store` value to Wasm as function argument");
                    }

                    if $t::valtype().is_ref() {
                        let param_ty = _func_ty.param(_i).unwrap();
                        let ref_ty = param_ty.unwrap_ref();
                        if ref_ty.heap_type.is_concrete() {
                            $t.dynamic_concrete_type_check(_store, ref_ty.nullable, &ref_ty.heap_type)?;
                        }
                    }

                    // Safety: the macro guarantees that `Self::VMValStorage` has enough space
                    let dst = unsafe { _dst.map(|p| &raw mut (*p)[_i]) };
                    $t.store(_store, dst)?;

                    _i += 1;
                )*
                Ok(())
            }
        }
    }
}

macro_rules! impl_wasm_results {
    ($n:tt $($t:ident)*) => {
        #[allow(non_snake_case, reason = "argument names above are uppercase")]
        #[allow(clippy::unused_unit, reason = "the empty tuple case generates an empty tuple as return value, which makes clippy mad but thats fine")]
        // Safety: see `WasmTy` for details
        unsafe impl<$($t: WasmTy,)*> WasmResults for ($($t,)*) {
            unsafe fn load(_store: &mut Store, _abi: &Self::VMValStorage) -> Self {
                let [$($t,)*] = _abi;
                // Safety: ensured by caller
                ($(unsafe { $t::load(_store, $t) },)*)
            }
        }
    }
}

for_each_function_signature!(impl_wasm_params);
for_each_function_signature!(impl_wasm_results);

// Forwards from a bare type `T` to the 1-tuple type `(T,)`
// Safety: see `impl_wasm_params!`
unsafe impl<T: WasmTy> WasmParams for T {
    type VMValStorage = <(T,) as WasmParams>::VMValStorage;

    fn typecheck<'a>(params: impl ExactSizeIterator<Item = &'a WasmValType>) -> wasm::Result<()> {
        <(T,) as WasmParams>::typecheck(params)
    }

    fn store(
        self,
        store: &mut Store,
        func_ty: &FuncType,
        dst: &mut MaybeUninit<Self::VMValStorage>,
    ) -> wasm::Result<()> {
        <(T,) as WasmParams>::store((self,), store, func_ty, dst)
    }
}

// Forwards from a bare type `T` to the 1-tuple type `(T,)`
// Safety: see `impl_wasm_results!`
unsafe impl<T: WasmTy> WasmResults for T {
    unsafe fn load(store: &mut Store, abi: &Self::VMValStorage) -> Self {
        // Safety: ensured by caller
        unsafe { <(T,) as WasmResults>::load(store, abi).0 }
    }
}

fn enter_wasm(vmctx: *mut VMContext, offsets: &StaticVMOffsets) -> WasmExecutionGuard {
    let stack_pointer = arch::get_stack_pointer();
    let wasm_stack_limit = stack_pointer.checked_sub(MAX_WASM_STACK).unwrap();

    // Safety: at this point the `VMContext` is initialized and accessing its fields is safe.
    unsafe {
        let stack_limit_ptr = vmctx
            .byte_add(offsets.vmctx_stack_limit() as usize)
            .cast::<usize>();
        let prev_stack = mem::replace(&mut *stack_limit_ptr, wasm_stack_limit);
        WasmExecutionGuard {
            stack_limit_ptr,
            prev_stack,
        }
    }
}

struct WasmExecutionGuard {
    stack_limit_ptr: *mut usize,
    prev_stack: usize,
}

impl Drop for WasmExecutionGuard {
    fn drop(&mut self) {
        // Safety: this relies on `enter_wasm` correctly calculating the stack limit pointer.
        unsafe {
            *self.stack_limit_ptr = self.prev_stack;
        }
    }
}
