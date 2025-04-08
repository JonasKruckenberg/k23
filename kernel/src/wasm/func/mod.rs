// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod host;
mod typed;

use crate::wasm::indices::VMSharedTypeIndex;
use crate::wasm::store::{StoreOpaque, Stored};
use crate::wasm::types::FuncType;
use crate::wasm::values::Val;
use crate::wasm::vm::{ExportedFunction, VMArrayCallHostFuncContext, VMFuncRef, VMFunctionImport, VMOpaqueContext, VMVal, VmPtr};
use alloc::boxed::Box;
use alloc::sync::Arc;
use core::ffi::c_void;
use core::ptr::NonNull;
use crate::wasm::{Module, Store};

pub use host::{HostFunc, IntoFunc, HostParams, HostResults};
pub use typed::{TypedFunc, WasmResults, WasmParams, WasmTy};

#[derive(Clone, Copy, Debug)]
pub struct Func(pub(super) Stored<FuncData>);
#[derive(Debug)]
pub struct FuncData {
    kind: FuncKind,
}
unsafe impl Send for FuncData {}
unsafe impl Sync for FuncData {}

#[derive(Debug)]

enum FuncKind {
    StoreOwned { export: ExportedFunction },
    SharedHost(Arc<HostFunc>),
    Host(Box<HostFunc>),
}

impl Func {
    pub fn wrap<T, Params, Results>(
        store: &mut Store<T>,
        func: impl IntoFunc<T, Params, Results>,
    ) -> TypedFunc<Params, Results>
    where
        Params: WasmParams,
        Results: WasmResults,
    {
        let (func, ty) = HostFunc::wrap(store.engine(), func);

        let stored = store.add_function(FuncData {
            kind: FuncKind::Host(Box::new(func)),
        });

        unsafe { TypedFunc::new_unchecked(Self(stored), ty) }
    }

    pub fn typed<Params, Results>(
        self,
        store: &StoreOpaque,
    ) -> crate::Result<TypedFunc<Params, Results>>
    where
        Params: WasmParams,
        Results: WasmResults,
    {
        let ty = self.ty(store);
        Params::typecheck(store.engine(), ty.params())?;
        Results::typecheck(store.engine(), ty.results())?;

        Ok(unsafe { TypedFunc::new_unchecked(self, ty) })
    }

    pub async fn call(
        self,
        store: &mut StoreOpaque,
        params: &[Val],
        results: &mut [Val],
    ) -> crate::Result<()> {
        todo!()
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
    pub unsafe fn call_unchecked(
        self,
        store: &mut StoreOpaque,
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
        for (arg, slot) in params.iter().zip(&mut values_vec) {
            unsafe {
                *slot = arg.to_vmval(store)?;
            }
        }

        // Safety: func refs obtained from our store are always usable by us.
        let func_ref = unsafe { self.vm_func_ref(store).as_ref() };

        // do the actual call
        // Safety: at this point we have typechecked, we have allocated enough memory for the params
        // and results, and obtained a valid func ref to call.
        unsafe {
            do_call(store, func_ref, &mut values_vec)?;
        }

        // copy the results out of the storage
        let func_ty = self.ty(store);
        for ((i, slot), vmval) in results.iter_mut().enumerate().zip(&values_vec) {
            let ty = func_ty.result(i).unwrap();
            // Safety: caller has to ensure safety
            *slot = unsafe { Val::from_vmval(store, *vmval, ty) };
        }

        // clean up and return the argument storage
        values_vec.truncate(0);
        store.return_wasm_vmval_storage(values_vec);

        Ok(())
    }

    pub fn ty(&self, store: &StoreOpaque) -> FuncType {
        FuncType::from_shared_type_index(store.engine(), self.type_index(store))
    }

    pub fn matches_ty(&self, store: &StoreOpaque, ty: FuncType) -> bool {
        let actual_ty = self.ty(store);
        actual_ty.matches(&ty)
    }

    pub(super) fn type_index(&self, store: &StoreOpaque) -> VMSharedTypeIndex {
        unsafe { self.vm_func_ref(store).as_ref().type_index }
    }

    pub(super) unsafe fn from_exported_function(
        store: &mut StoreOpaque,
        export: ExportedFunction,
    ) -> Self {
        let stored = store.add_function(FuncData {
            kind: FuncKind::StoreOwned { export },
        });
        Self(stored)
    }

    pub(super) fn as_vmfunction_import(&self, store: &mut StoreOpaque, module: &Module) -> VMFunctionImport {
        let f = self.vm_func_ref(store);

        unsafe {
            VMFunctionImport {
                wasm_call: f.as_ref().wasm_call.unwrap_or_else(|| {
                    // Assert that this is a array-call function, since those
                    // are the only ones that could be missing a `wasm_call`
                    // trampoline.
                    let _ = VMArrayCallHostFuncContext::from_opaque(f.as_ref().vmctx.as_non_null());

                    let sig = self.type_index(store);
                    
                    let ptr = module.wasm_to_array_trampoline(sig).expect(
                        "if the wasm is importing a function of a given type, it must have the \
                         type's trampoline",
                    );
                    
                    VmPtr::from(ptr)
                }),
                array_call: f.as_ref().array_call,
                vmctx: f.as_ref().vmctx,
            }
        }
    }

    pub(super) fn comes_from_same_store(&self, store: &StoreOpaque) -> bool {
        store.has_function(self.0)
    }

    pub(super) unsafe fn from_vm_func_ref(
        store: &mut StoreOpaque,
        func_ref: NonNull<VMFuncRef>,
    ) -> Self {
        unsafe {
            debug_assert!(func_ref.as_ref().type_index != VMSharedTypeIndex::default());
            Func::from_exported_function(store, ExportedFunction { func_ref })
        }
    }

    pub(super) fn vm_func_ref(&self, store: &StoreOpaque) -> NonNull<VMFuncRef> {
        match &store[self.0].kind {
            FuncKind::StoreOwned { export } => export.func_ref,
            FuncKind::SharedHost(func) => NonNull::from(func.func_ref()),
            FuncKind::Host(func) => NonNull::from(func.func_ref()),
        }
    }

    pub(super) unsafe fn from_vmval(store: &mut StoreOpaque, raw: *mut c_void) -> Option<Self> {
        unsafe { Some(Func::from_vm_func_ref(store, NonNull::new(raw.cast())?)) }
    }

    /// Extracts the raw value of this `Func`, which is owned by `store`.
    ///
    /// This function returns a value that's suitable for writing into the
    /// `funcref` field of the [`ValRaw`] structure.
    ///
    /// # Safety
    ///
    /// The returned value is only valid for as long as the store is alive and
    /// this function is properly rooted within it. Additionally this function
    /// should not be liberally used since it's a very low-level knob.
    pub(super) unsafe fn to_vmval(&self, store: &mut StoreOpaque) -> *mut c_void {
        self.vm_func_ref(store).as_ptr().cast()
    }
}

pub(super) unsafe fn do_call(
    store: &mut StoreOpaque,
    func_ref: &VMFuncRef,
    params_and_results: &mut [VMVal],
) -> crate::Result<()> {
    // Safety: TODO
    unsafe {
        let vmctx = VMOpaqueContext::from_vmcontext(store.default_caller());

        // TODO catch traps
        func_ref.array_call(vmctx, NonNull::from(params_and_results));

        todo!()

        // let _guard = WasmExecutionGuard::enter_wasm(vmctx, &module.offsets().static_);
        //
        // let span = tracing::trace_span!("WASM");
        //
        // let res = span.in_scope(|| {
        //     super::trap_handler::catch_traps(vmctx, module.offsets().static_.clone(), || {
        //         arch::array_call(
        //             func_ref,
        //             vmctx,
        //             vmctx,
        //             params_and_results.as_mut_ptr(),
        //             params_and_results.len(),
        //         );
        //     })
        // });
        //
        // match res {
        //     Ok(_)
        //     // The userspace ABI uses the Trap::Exit code to signal a graceful exit
        //     | Err((Trap::Exit, _)) => Ok(()),
        //     Err((trap, backtrace)) => bail!("WebAssembly call failed with error:\n{:?}\n{:?}", trap, backtrace),
        // }
    }
}
