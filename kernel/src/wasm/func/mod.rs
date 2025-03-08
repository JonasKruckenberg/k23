// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod host;
mod typed;

use crate::util::send_sync_ptr::SendSyncPtr;
use crate::util::zip_eq::IteratorExt;
use crate::wasm::indices::{CanonicalizedTypeIndex, VMSharedTypeIndex};
use crate::wasm::runtime::{StaticVMOffsets, VMFunctionImport};
use crate::wasm::store::Stored;
use crate::wasm::translate::{
    Finality, WasmCompositeType, WasmCompositeTypeInner, WasmFuncType, WasmSubType, WasmValType,
};
use crate::wasm::type_registry::RegisteredType;
use crate::wasm::{runtime, Engine, Error, Module, Store, Trap, VMContext, VMFuncRef, VMVal, Val, MAX_WASM_STACK};
use crate::{arch, ensure, wasm};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::fmt::Write;
use core::mem;
use core::ptr::NonNull;
use fallible_iterator::FallibleIterator;

pub use host::{HostContext, HostFunc, IntoFunc};
pub use typed::{TypedFunc, WasmParams, WasmResults, WasmTy};

/// A WebAssembly function type.
///
/// This is essentially a reference counted index into the engine's type registry.
#[derive(Debug, Clone)]
pub struct FuncType(RegisteredType);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Func(pub(super) Stored<FuncData>);

#[derive(Debug)]
pub struct FuncData {
    kind: FuncKind,
    // This is somewhat expensive to load from the `Engine` and in most
    // optimized use cases (e.g. `TypedFunc`) it's not actually needed or it's
    // only needed rarely. To handle that this is an optionally-contained field
    // which is lazily loaded into as part of `Func::call`.
    //
    // Also note that this is intentionally placed behind a pointer to keep it
    // small as `FuncData` instances are often inserted into a `Store`.
    ty: Option<Box<FuncType>>,
}

#[derive(Debug)]
enum FuncKind {
    StoreOwned { export: runtime::ExportedFunction },
    SharedHost(Arc<HostFunc>),
    Host(Box<HostFunc>),
}

// === impl Func ===

impl Func {
    pub fn wrap<Params, Results>(store: &mut Store, func: impl IntoFunc<Params, Results>) -> Self {
        let (func, ty) = HostFunc::wrap(store, func);

        Self(store.push_function(FuncData {
            kind: FuncKind::Host(Box::new(func)),
            ty: Some(Box::new(ty)),
        }))
    }

    /// Translation between Rust types and WebAssembly types looks like:
    ///
    /// | WebAssembly                               | Rust                                  |
    /// |-------------------------------------------|---------------------------------------|
    /// | `i32`                                     | `i32` or `u32`                        |
    /// | `i64`                                     | `i64` or `u64`                        |
    /// | `f32`                                     | `f32`                                 |
    /// | `f64`                                     | `f64`                                 |
    /// | `externref` aka `(ref null extern)`       | `Option<Rooted<ExternRef>>`           |
    /// | `(ref extern)`                            | `Rooted<ExternRef>`                   |
    /// | `nullexternref` aka `(ref null noextern)` | `Option<NoExtern>`                    |
    /// | `(ref noextern)`                          | `NoExtern`                            |
    /// | `anyref` aka `(ref null any)`             | `Option<Rooted<AnyRef>>`              |
    /// | `(ref any)`                               | `Rooted<AnyRef>`                      |
    /// | `eqref` aka `(ref null eq)`               | `Option<Rooted<EqRef>>`               |
    /// | `(ref eq)`                                | `Rooted<EqRef>`                       |
    /// | `i31ref` aka `(ref null i31)`             | `Option<I31>`                         |
    /// | `(ref i31)`                               | `I31`                                 |
    /// | `structref` aka `(ref null struct)`       | `Option<Rooted<StructRef>>`           |
    /// | `(ref struct)`                            | `Rooted<StructRef>`                   |
    /// | `arrayref` aka `(ref null array)`         | `Option<Rooted<ArrayRef>>`            |
    /// | `(ref array)`                             | `Rooted<ArrayRef>`                    |
    /// | `nullref` aka `(ref null none)`           | `Option<NoneRef>`                     |
    /// | `(ref none)`                              | `NoneRef`                             |
    /// | `funcref` aka `(ref null func)`           | `Option<Func>`                        |
    /// | `(ref func)`                              | `Func`                                |
    /// | `(ref null <func type index>)`            | `Option<Func>`                        |
    /// | `(ref <func type index>)`                 | `Func`                                |
    /// | `nullfuncref` aka `(ref null nofunc)`     | `Option<NoFunc>`                      |
    /// | `(ref nofunc)`                            | `NoFunc`                              |
    /// | `v128`                                    | `V128` on `x86-64` and `aarch64` only |
    ///
    /// (Note that this mapping is the same as that of [`Func::wrap`], and that
    /// anywhere a `Rooted<T>` appears, a `ManuallyRooted<T>` may also appear).
    pub fn typed<Params, Results>(
        self,
        store: &mut Store,
    ) -> wasm::Result<TypedFunc<Params, Results>>
    where
        Params: WasmParams,
        Results: WasmResults,
    {
        let ty = self.ty(store);
        let wasm_ty = ty.as_wasm_func_type();
        Params::typecheck(wasm_ty.params.iter())?;
        Results::typecheck(wasm_ty.results.iter())?;

        // Safety: we have checked the parameter and result types above
        Ok(unsafe { TypedFunc::new_unchecked(self, ty.clone()) })
    }

    pub fn ty<'s>(self, store: &'s mut Store) -> &'s FuncType {
        if store[self.0].ty.is_none() {
            // Safety: An FuncRef we get from the store is well initialized
            let func_ref = unsafe { self.as_vm_func_ref(store).as_ref() };

            let ty = store
                .engine
                .type_registry()
                .get_type(&store.engine, func_ref.type_index)
                .unwrap();

            store[self.0].ty = Some(Box::new(FuncType(ty)));
        }

        // Safety: if ty was none, we definitely initialized it above
        unsafe { store[self.0].ty.as_deref().unwrap_unchecked() }
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
    ) -> super::Result<()> {
        store
            .on_fiber(|store| {
                // Do the typechecking. Notice how `TypedFunc::call` is essentially the same function
                // minus this typechecking? Yeah. That's the benefit of the typed function.
                let ty = self.ty(store);
                let ty = ty.as_wasm_func_type();

                let mut params_ = ty.params.iter().zip_eq(params);
                while let Some((expected, param)) =
                    params_.next().map_err(|_| Error::MismatchedTypes)?
                {
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
    ) -> super::Result<()> {
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
        let ty = self.ty(store).clone();
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
        Self(store.push_function(FuncData {
            kind: FuncKind::StoreOwned { export },
            ty: None,
        }))
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

    pub(super) fn as_vmfunction_import(&self, store: &Store, module: &Module) -> VMFunctionImport {
        // Safety: An FuncRef we get from the store is well initialized
        let func_ref = unsafe { self.as_vm_func_ref(store).as_ref() };
        VMFunctionImport {
            wasm_call: if let Some(wasm_call) = func_ref.wasm_call {
                wasm_call
            } else {
                module.wasm_to_array_trampoline(func_ref.type_index).unwrap()
            },
            array_call: func_ref.array_call,
            vmctx: func_ref.vmctx,
        }
    }

    pub(super) fn as_raw(&self, store: &mut Store) -> *mut c_void {
        self.as_vm_func_ref(store).as_ptr().cast()
    }

    fn as_vm_func_ref(self, store: &Store) -> NonNull<VMFuncRef> {
        match store[self.0].kind {
            FuncKind::StoreOwned { export } => export.func_ref,
            FuncKind::SharedHost(ref func) => func.exported_func().func_ref,
            FuncKind::Host(ref func) => func.exported_func().func_ref,
        }
    }
}

unsafe fn do_call(
    store: &mut Store,
    func_ref: &VMFuncRef,
    params_and_results: &mut [VMVal],
) -> super::Result<()> {
    // Safety: TODO
    unsafe {
        let vmctx = VMContext::from_opaque(func_ref.vmctx);
        let module = store[store.get_instance_from_vmctx(vmctx)].module();

        let _guard = WasmExecutionGuard::enter_wasm(vmctx, &module.offsets().static_);

        let span = tracing::trace_span!("WASM");

        let res = span.in_scope(|| {
            super::trap_handler::catch_traps(vmctx, module.offsets().clone(), || {
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
            | Err(Error::Trap {
                      trap: Trap::Exit, ..
                  }) => Ok(()),
            Err(err) => Err(err),
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
    pub fn new(
        engine: &Engine,
        params: impl IntoIterator<Item = WasmValType>,
        results: impl IntoIterator<Item = WasmValType>,
    ) -> Self {
        Self::with_finality_and_supertype(engine, Finality::Final, None, params, results)
            .expect("cannot fail without a supertype")
    }

    pub fn with_finality_and_supertype(
        engine: &Engine,
        finality: Finality,
        supertype: Option<&Self>,
        params: impl IntoIterator<Item = WasmValType>,
        results: impl IntoIterator<Item = WasmValType>,
    ) -> wasm::Result<Self> {
        let params = params.into_iter();
        let results = results.into_iter();

        let mut wasmtime_params = Vec::with_capacity({
            let size_hint = params.size_hint();
            let cap = size_hint.1.unwrap_or(size_hint.0);
            // Only reserve space if we have a supertype, as that is the only time
            // that this vec is used.
            supertype.is_some() as usize * cap
        });

        let mut wasmtime_results = Vec::with_capacity({
            let size_hint = results.size_hint();
            let cap = size_hint.1.unwrap_or(size_hint.0);
            // Same as above.
            supertype.is_some() as usize * cap
        });

        let to_wasm_type = |ty: WasmValType, vec: &mut Vec<_>| {
            if supertype.is_some() {
                vec.push(ty.clone());
            }

            ty
        };

        let wasm_func_ty = WasmFuncType {
            params: params
                .map(|p| to_wasm_type(p, &mut wasmtime_params))
                .collect(),
            results: results
                .map(|r| to_wasm_type(r, &mut wasmtime_results))
                .collect(),
        };

        if let Some(supertype) = supertype {
            ensure!(
                supertype.finality().is_non_final(),
                Error::MismatchedTypes,
                "cannot create a subtype of a final supertype"
            );
            ensure!(
                Self::matches_impl(
                    wasmtime_params.iter().cloned(),
                    supertype.params().cloned(),
                    wasmtime_results.iter().cloned(),
                    supertype.results().cloned()
                ),
                Error::MismatchedTypes,
                "function type must match its supertype: found (func{params:?}{results}), expected \
                 {supertype:?}",
                params = if wasmtime_params.is_empty() {
                    String::new()
                } else {
                    let mut s = " (params".to_string();
                    for p in &wasmtime_params {
                        write!(&mut s, " {p}").unwrap();
                    }
                    s.push(')');
                    s
                },
                results = if wasmtime_results.is_empty() {
                    String::new()
                } else {
                    let mut s = format!(" (results");
                    for r in &wasmtime_results {
                        write!(&mut s, " {r}").unwrap();
                    }
                    s.push(')');
                    s
                },
            );
        }

        Ok(Self::from_wasm_func_type(
            engine,
            finality.is_final(),
            supertype.map(|ty| ty.type_index().into()),
            wasm_func_ty,
        ))
    }

    fn from_wasm_func_type(
        engine: &Engine,
        is_final: bool,
        supertype: Option<CanonicalizedTypeIndex>,
        ty: WasmFuncType,
    ) -> FuncType {
        let registered_type = engine.type_registry().register_type(
            engine,
            WasmSubType {
                is_final,
                supertype,
                composite_type: WasmCompositeType {
                    shared: false,
                    inner: WasmCompositeTypeInner::Func(ty),
                },
            },
        );

        Self(registered_type)
    }

    /// Does this function type match the other function type?
    ///
    /// That is, is this function type a subtype of the other function type?
    ///
    /// # Panics
    ///
    /// Panics if either type is associated with a different engine from the
    /// other.
    pub fn matches(&self, other: &FuncType) -> bool {
        // Avoid matching on structure for subtyping checks when we have
        // precisely the same type.
        if self.type_index() == other.type_index() {
            return true;
        }

        Self::matches_impl(
            self.params().cloned(),
            other.params().cloned(),
            self.results().cloned(),
            other.results().cloned(),
        )
    }

    fn matches_impl(
        a_params: impl ExactSizeIterator<Item = WasmValType>,
        b_params: impl ExactSizeIterator<Item = WasmValType>,
        a_results: impl ExactSizeIterator<Item = WasmValType>,
        b_results: impl ExactSizeIterator<Item = WasmValType>,
    ) -> bool {
        a_params.len() == b_params.len()
            && a_results.len() == b_results.len()
            // Params are contravariant and results are covariant. For more
            // details and a refresher on variance, read
            // https://github.com/bytecodealliance/wasm-tools/blob/f1d89a4/crates/wasmparser/src/readers/core/types/matches.rs#L137-L174
            && a_params
            .zip(b_params)
            .all(|(a, b)| b.matches(&a))
            && a_results
            .zip(b_results)
            .all(|(a, b)| a.matches(&b))
    }

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

    fn params(&self) -> impl ExactSizeIterator<Item = &'_ WasmValType> + Sized {
        self.0.unwrap_func().params.iter()
    }
    fn results(&self) -> impl ExactSizeIterator<Item = &'_ WasmValType> + Sized {
        self.0.unwrap_func().results.iter()
    }

    fn finality(&self) -> Finality {
        if self.0.is_final {
            Finality::Final
        } else {
            Finality::NonFinal
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
pub(self) use for_each_function_signature;
