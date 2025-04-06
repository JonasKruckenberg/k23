// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch;
use crate::util::zip_eq::IteratorExt;
use crate::wasm::indices::VMSharedTypeIndex;
use crate::wasm::runtime::{StaticVMOffsets, VMFunctionImport};
use crate::wasm::store::Stored;
use crate::wasm::translate::{WasmFuncType, WasmHeapType, WasmRefType, WasmValType};
use crate::wasm::type_registry::RegisteredType;
use crate::wasm::{MAX_WASM_STACK, Store, Trap, VMContext, VMFuncRef, VMVal, Val, runtime};
use anyhow::{bail, ensure};
use core::ffi::c_void;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::{mem, ptr};
use fallible_iterator::FallibleIterator;

pub struct TypedFunc<Params, Results> {
    ty: FuncType,
    func: Func,
    _m: PhantomData<fn(Params) -> Results>,
}

#[derive(Debug, Clone, Copy)]
pub struct Func(Stored<runtime::ExportedFunction>);

/// A WebAssembly function type.
///
/// This is essentially a reference counted index into the engine's type registry.
pub struct FuncType(RegisteredType);

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
    /// Return this values type.
    fn valtype() -> WasmValType;
    /// Store the values lowered representation into the given `slot`.
    fn store(self, store: &mut Store, slot: &mut MaybeUninit<VMVal>) -> crate::Result<()>;
    /// Load the value from its lowered representation.
    unsafe fn load(store: &mut Store, ptr: &VMVal) -> Self;
    /// Is the value compatible with the given store?
    ///
    /// Numbers (`I32`, `I64`, `F32`, `F64`, and `V128`) are store agnostic and can be used
    /// across stores, while all references are tied to their owning store (they are just pointers
    /// into store-owned memory after all) and *cannot* be used outside their owning store.
    fn compatible_with_store(&self, store: &Store) -> bool;

    fn dynamic_concrete_type_check(
        &self,
        store: &Store,
        nullable: bool,
        actual: &WasmHeapType,
    ) -> crate::Result<()>;

    /// Assert this value is of the `expected` type.
    #[inline]
    fn typecheck(actual: &WasmValType, position: TypeCheckPosition) -> crate::Result<()> {
        let expected = Self::valtype();

        match position {
            TypeCheckPosition::Result => expected.ensure_matches(actual),
            TypeCheckPosition::Param => match (expected.get_ref(), actual.get_ref()) {
                (Some(expected_ref), Some(actual_ref)) if actual_ref.heap_type.is_concrete() => {
                    expected_ref
                        .heap_type
                        .top()
                        .ensure_matches(&actual_ref.heap_type.top())
                }
                _ => expected.ensure_matches(actual),
            },
        }
    }
}

/// A type that can be used as an argument for WASM functions.
///
/// This trait is implemented for bare types that may be passed to WASM and tuples of those types.
///
/// # Safety
///
/// This trait should not be implemented manually.
pub unsafe trait WasmParams: Send {
    /// The storage for holding the [`VMVal`] parameters and results
    ///
    /// The storage for holding the array-call parameters and results.
    /// This should most likely be a `[VMVal; N]` array where `N` is the number of parameters.
    type VMValStorage: Copy;

    /// Assert that the provided types are compatible with this type.
    ///
    /// # Errors
    ///
    /// This should return an error IF
    /// - The number of provided types does not *exactly* match the number of expected types.
    /// - The type does not match its expected type.
    fn typecheck<'a>(params: impl ExactSizeIterator<Item = &'a WasmValType>) -> crate::Result<()>;

    /// Stores this types lowered representation into the provided buffer.
    fn store(
        self,
        store: &mut Store,
        func_ty: &FuncType,
        dst: &mut MaybeUninit<Self::VMValStorage>,
    ) -> crate::Result<()>;
}

/// A type that may be returned from WASM functions.
///
/// This trait is implemented for bare types that may be passed to WASM and tuples of those types.
///
/// Note: This type piggybacks off of the [`WasmParams`] trait as all types implement both
/// [`WasmParams`] AND [`WasmResults`] we can just reuse [`WasmParams`] typecheck and storage
/// implementations.
///
/// # Safety
///
/// This trait should not be implemented manually.
pub unsafe trait WasmResults: WasmParams {
    /// Loads type from its lowered representation.
    unsafe fn load(store: &mut Store, abi: &Self::VMValStorage) -> Self;
}

// === impl TypedFunc ===

impl<Params, Results> TypedFunc<Params, Results>
where
    Params: WasmParams,
    Results: WasmResults,
{
    /// Invokes this functions with the `params`, returning the results asynchronously.
    ///
    /// Execution of WebAssembly will happen synchronously in the `poll` method of the
    /// future returned from this function. Note that, the faster `poll` methods return the more
    /// responsive the overall system is, so WebAssembly execution interruption should be configured
    /// such that this futures `poll` method resolves *reasonably* quickly.
    /// (Reasonably because at the end of the day a task will need to block if it wants to perform
    /// any meaningful work, there is no way around it).
    ///
    /// This function will return `Ok(())` if execution completed without a trap
    /// or error of any kind. In this situation the results will be written to
    /// the provided `results` array.
    ///
    /// # Errors
    ///
    /// Any error which occurs throughout the execution of the function will be
    /// returned as `Err(e)`.
    /// Errors typically indicate that execution of WebAssembly was halted
    /// mid-way and did not complete after the error condition happened.
    pub async fn call(self, store: &mut Store, params: Params) -> crate::Result<Results> {
        store
            .on_fiber(|store| {
                #[cfg(debug_assertions)]
                Self::debug_typecheck(self.ty.as_wasm_func_type());

                self.call_inner(store, params)
            })
            .await?
    }

    fn call_inner(&self, store: &mut Store, params: Params) -> crate::Result<Results> {
        // Safety: The code below does the lowering and lifting of VM values. The correctness of which is enforced
        // both through this type's generics AND the correct implementation of the array-call trampoline.
        unsafe {
            // Okay, so what is going on here?
            //
            // Because the `TypedFunc` generics we know exactly how much space we need for the
            // array-call parameters and results *at compile time* (declared by the `WasmParams::VMValStorage` or
            // `WasmResults::VMValStorage` associated types). Which neatly allows us to allocate that array on the stack.
            //
            // Remember that the array-call "calling convention" will store the parameters in a consecutive
            // array and use *that same array* to store the results too. This means before jumping to
            // WASM we essentially have a `Params::VMValStorage` on stack, while after returning we
            // have a `Results::VMValStorage` on stack. Note also that (just like when calling through
            // the untyped func) we need to make sure to reserve enough space to fit the larger of the two types.
            //
            // If that sounded suspiciously like a union, congrats! You are correct, a union is exactly
            // what we use.
            //
            // Notice how `Func::call_inner` uses a very different approach: Instead of allocating on
            // stack, it has to use a heap-allocated vec that it needs to resize appropriately.
            union Storage<T: Copy, U: Copy> {
                params: MaybeUninit<T>,
                results: U,
            }

            // Allocate the space on stack
            let mut storage = Storage::<Params::VMValStorage, Results::VMValStorage> {
                params: MaybeUninit::uninit(),
            };

            // Aaand initialize it by lowering the params into it
            params.store(store, &self.ty, &mut storage.params)?;

            // Now, heres a tricky part: We allocated a `Storage::<Params::VMValStorage, Results::VMValStorage>`
            // type on stack above. But what we need for the array call is a `*mut [VMVal]`. We therefore
            // need to essentially "transmute" the union into a slice of `VMVal` which is exactly what
            // we do below.
            {
                let storage_len = size_of_val::<Storage<_, _>>(&storage) / size_of::<VMVal>();
                let storage: *mut Storage<_, _> = &mut storage;
                let storage = storage.cast::<VMVal>();
                let storage = core::slice::from_raw_parts_mut(storage, storage_len);

                let func_ref = self.func.as_vm_func_ref(store).as_ref();

                // now do the actual WASM calling and trap catching business
                do_call(store, func_ref, storage)?;
            }

            // At this point we have successfully returned from WASM which means that now we have a
            // `Results::VMValStorage` on stack instead of the `Params::VMValStorage` that we wrote into it.
            // Go ahead and lift our Rust values from it and return.
            Ok(Results::load(store, &storage.results))
        }
    }

    #[cfg(debug_assertions)]
    fn debug_typecheck(ty: &WasmFuncType) {
        Params::typecheck(ty.params.iter()).expect("params should match");
        Results::typecheck(ty.results.iter()).expect("results should match");
    }
}

// === impl Func ===

impl Func {
    pub fn typed<Params, Results>(self, store: &Store) -> crate::Result<TypedFunc<Params, Results>>
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

    pub fn ty(self, store: &Store) -> FuncType {
        // Safety: An FuncRef we get from the store is well initialized
        let func_ref = unsafe { store[self.0].func_ref.as_ref() };
        let ty = store
            .engine
            .type_registry()
            .get_type(&store.engine, func_ref.type_index)
            .unwrap();
        FuncType(ty)
    }

    pub fn comes_from_same_store(self, store: &Store) -> bool {
        store.has_function(self.0)
    }

    /// Invokes this function with the `params` given and writes returned values
    /// to `results`.
    ///
    /// The `params` here must match the type signature of this `Func`, or an
    /// error will occur. Additionally, `results` must have the same
    /// length as the number of results for this function.
    ///
    /// Execution of WebAssembly will happen synchronously in the `poll` method of the
    /// future returned from this function. Note that, the faster `poll` methods return the more
    /// responsive the overall system is, so WebAssembly execution interruption should be configured
    /// such that this futures `poll` method resolves *reasonably* quickly.
    /// (Reasonably because at the end of the day a task will need to block if it wants to perform
    /// any meaningful work, there is no way around it).
    ///
    /// This function will return `Ok(())` if execution completed without a trap
    /// or error of any kind. In this situation the results will be written to
    /// the provided `results` array.
    ///
    /// # Errors
    ///
    /// Any error which occurs throughout the execution of the function will be
    /// returned as `Err(e)`.
    /// Errors typically indicate that execution of WebAssembly was halted
    /// mid-way and did not complete after the error condition happened.
    pub async fn call(
        self,
        store: &mut Store,
        params: &[Val],
        results: &mut [Val],
    ) -> crate::Result<()> {
        store
            .on_fiber(|store| {
                // Do the typechecking. Notice how `TypedFunc::call` is essentially the same function
                // minus this typechecking? Yeah. That's the benefit of the typed function.
                let ty = self.ty(store);
                let ty = ty.as_wasm_func_type();

                let mut params_ = ty.params.iter().zip_eq(params);
                while let Some((expected, param)) = params_.next()? {
                    let found = param.ty();
                    ensure!(
                        *expected == found,
                        "Type mismatch. Expected `{expected:?}`, but found `{found:?}`"
                    );
                }

                ensure!(
                    results.len() >= ty.results.len(),
                    "Results slice too small. Need space for at least {}, but got only {}",
                    ty.results.len(),
                    results.len()
                );

                // Safety: we have checked the types above, we're safe to proceed
                unsafe { self.call_inner(store, params, results) }
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
    unsafe fn call_inner(
        self,
        store: &mut Store,
        params: &[Val],
        results: &mut [Val],
    ) -> crate::Result<()> {
        // This function mainly performs the lowering and lifting of VMVal values from and to Rust.
        // Because - unlike TypedFunc - we don't have compile-time knowledge about the function type,
        // we use a heap allocated vec (obtained through `store.take_wasm_vmval_storage()`) to store
        // our parameters into and read results from.
        //
        // This is obviously a little less efficient, but it's not that big of a deal.

        // take out the argument storage from the store
        let mut values_vec = store.take_wasm_vmval_storage();
        debug_assert!(values_vec.is_empty());

        // resize the vec so we can be sure that params and results will fit.
        let values_vec_size = params.len().max(results.len());
        values_vec.resize_with(values_vec_size, || VMVal::v128(0));

        // copy the arguments into the storage vec
        for (arg, slot) in params.iter().copied().zip(&mut values_vec) {
            *slot = arg.as_vmval(store);
        }

        // Safety: func refs obtained from our store are always usable by us.
        let func_ref = unsafe { self.as_vm_func_ref(store).as_ref() };

        // do the actual call
        // Safety: at this point we have typechecked, we have allocated enough memory for the params
        // and results, and obtained a valid func ref to call.
        unsafe {
            do_call(store, func_ref, &mut values_vec)?;
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

    /// # Safety
    ///
    /// The caller must ensure `export` is a valid exported global within `store`.
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

    pub(super) fn as_vmfunction_import(&self, store: &Store) -> VMFunctionImport {
        // Safety: An FuncRef we get from the store is well initialized
        let func_ref = unsafe { self.as_vm_func_ref(store).as_ref() };
        VMFunctionImport {
            wasm_call: func_ref.wasm_call,
            array_call: func_ref.array_call,
            vmctx: func_ref.vmctx,
        }
    }

    pub(super) fn as_raw(&self, store: &mut Store) -> *mut c_void {
        store[self.0].func_ref.as_ptr().cast()
    }

    fn as_vm_func_ref(self, store: &Store) -> NonNull<VMFuncRef> {
        store[self.0].func_ref
    }
}

unsafe fn do_call(
    store: &mut Store,
    func_ref: &VMFuncRef,
    params_and_results: &mut [VMVal],
) -> crate::Result<()> {
    // Safety: TODO
    unsafe {
        let vmctx = VMContext::from_opaque(func_ref.vmctx);
        let module = store[store.get_instance_from_vmctx(vmctx)].module();

        let _guard = WasmExecutionGuard::enter_wasm(vmctx, &module.offsets().static_);

        let span = tracing::trace_span!("WASM");

        let res = span.in_scope(|| {
            super::trap_handler::catch_traps(vmctx, module.offsets().static_.clone(), || {
                arch::array_call(
                    func_ref,
                    vmctx,
                    vmctx,
                    params_and_results.as_mut_ptr(),
                    params_and_results.len(),
                );
            })
        });

        match res {
            Ok(_)
            // The userspace ABI uses the Trap::Exit code to signal a graceful exit
            | Err((Trap::Exit, _)) => Ok(()),
            Err((trap, backtrace)) => bail!("WebAssembly call failed with error:\n{:?}\n{:?}", trap, backtrace),
        }
    }
}

struct WasmExecutionGuard {
    stack_limit_ptr: *mut usize,
    prev_stack: usize,
}

impl WasmExecutionGuard {
    fn enter_wasm(vmctx: *mut VMContext, offsets: &StaticVMOffsets) -> Self {
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
}

impl Drop for WasmExecutionGuard {
    fn drop(&mut self) {
        // Safety: this relies on `enter_wasm` correctly calculating the stack limit pointer.
        unsafe {
            *self.stack_limit_ptr = self.prev_stack;
        }
    }
}

// === impl FuncType ===

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

// === impl WasmTy ===

macro_rules! impl_wasm_ty_for_ints {
    ($($integer:ident/$get_integer:ident => $ty:ident)*) => ($(
        // Safety: this macro correctly delegates to the integer methods
        unsafe impl WasmTy for $integer {
            #[inline]
            fn valtype() -> WasmValType {
                WasmValType::$ty
            }

            #[inline]
            fn store(self, _store: &mut Store, ptr: &mut MaybeUninit<VMVal>) -> crate::Result<()> {
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
            ) -> crate::Result<()> {
                unreachable!("`dynamic_concrete_type_check` not implemented for integers");
            }

            #[inline]
            fn compatible_with_store(&self, _store: &Store) -> bool {
                true
            }
        }
    )*)
}

impl_wasm_ty_for_ints! {
    i32/get_i32 => I32
    i64/get_i64 => I64
    u32/get_u32 => I32
    u64/get_u64 => I64
}

macro_rules! impl_wasm_ty_for_floats {
    ($($float:ident/$get_float:ident => $ty:ident)*) => ($(
        // Safety: this macro correctly delegates to the float methods
        unsafe impl WasmTy for $float {
            #[inline]
            fn valtype() -> WasmValType {
                WasmValType::$ty
            }

            #[inline]
            fn store(self, _store: &mut Store, ptr: &mut MaybeUninit<VMVal>) -> crate::Result<()> {
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
            ) -> crate::Result<()> {
                unreachable!("`dynamic_concrete_type_check` not implemented for floats");
            }

            #[inline]
            fn compatible_with_store(&self, _store: &Store) -> bool {
                true
            }
        }
    )*)
}

impl_wasm_ty_for_floats! {
    f32/get_f32 => F32
    f64/get_f64 => F64
}

// Safety: functions are lowered as VMFuncRef pointers. TODO the correctness of this should be checked by tests
unsafe impl WasmTy for Func {
    fn valtype() -> WasmValType {
        WasmValType::Ref(WasmRefType::FUNCREF)
    }

    fn store(self, store: &mut Store, ptr: &mut MaybeUninit<VMVal>) -> crate::Result<()> {
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

    fn compatible_with_store(&self, store: &Store) -> bool {
        store.has_function(self.0)
    }

    fn dynamic_concrete_type_check(
        &self,
        _store: &Store,
        _nullable: bool,
        _actual: &WasmHeapType,
    ) -> crate::Result<()> {
        todo!()
    }
}

// Safety: functions are lowered as VMFuncRef pointers. TODO the correctness of this should be checked by tests
unsafe impl WasmTy for Option<Func> {
    fn valtype() -> WasmValType {
        WasmValType::Ref(WasmRefType::FUNCREF)
    }

    fn store(self, store: &mut Store, ptr: &mut MaybeUninit<VMVal>) -> crate::Result<()> {
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

    fn compatible_with_store(&self, store: &Store) -> bool {
        if let Some(f) = self {
            store.has_function(f.0)
        } else {
            true
        }
    }

    fn dynamic_concrete_type_check(
        &self,
        _store: &Store,
        _nullable: bool,
        _actual: &WasmHeapType,
    ) -> crate::Result<()> {
        todo!()
    }
}

// === impl WasmParams | WasmResults ===

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

            fn typecheck<'a>(mut params: impl ExactSizeIterator<Item = &'a WasmValType>) -> crate::Result<()> {
                let mut _n = 0;

                $(
                    match params.next() {
                        Some(t) => {
                            _n += 1;
                            $t::typecheck(t, TypeCheckPosition::Param)?
                        },
                        None => {
                            ::anyhow::bail!("expected {} types, found {}", $n as usize, params.len() + _n);
                        },
                    }
                )*

                match params.next() {
                    None => Ok(()),
                    Some(_) => {
                        _n += 1;
                        ::anyhow::bail!("expected {} types, found {}", $n, params.len() + _n);
                    },
                }
            }

            fn store(self, _store: &mut Store, _func_ty: &FuncType, _dst: &mut MaybeUninit<Self::VMValStorage>) -> crate::Result<()> {
                use $crate::util::maybe_uninit::MaybeUninitExt;

                let ($($t,)*) = self;
                let mut _i: usize = 0;

                $(
                    if !$t.compatible_with_store(_store) {
                        ::anyhow::bail!("attempt to pass cross-`Store` value to Wasm as function argument");
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

    fn typecheck<'a>(params: impl ExactSizeIterator<Item = &'a WasmValType>) -> crate::Result<()> {
        <(T,) as WasmParams>::typecheck(params)
    }

    fn store(
        self,
        store: &mut Store,
        func_ty: &FuncType,
        dst: &mut MaybeUninit<Self::VMValStorage>,
    ) -> crate::Result<()> {
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
