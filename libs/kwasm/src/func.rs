// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod host;
mod typed;

use alloc::boxed::Box;
use alloc::sync::Arc;
use core::pin::Pin;
use core::ptr::NonNull;

pub use host::{HostFunc, IntoFunc};
pub use typed::{TypedFunc, WasmParams, WasmResults, WasmTy};

use crate::func::host::Caller;
use crate::indices::VMSharedTypeIndex;
use crate::store::{StoreOpaque, Stored};
use crate::vm::{ExportedFunction, VMArrayCallHostFuncContext, VMFuncRef, VMFunctionImport, VmPtr};
use crate::{FuncType, Module, Store, Val};

#[derive(Clone, Copy, Debug)]
pub struct Func(pub(crate) Stored<FuncData>);

#[derive(Debug)]
pub struct FuncData {
    kind: FuncKind,
}

#[derive(Debug)]

enum FuncKind {
    StoreOwned { export: ExportedFunction },
    SharedHost(Arc<HostFunc>),
    Host(Box<HostFunc>),
}

// ===== impl Func =====

impl Func {
    pub fn new<T, F>(store: &mut Store<T>, ty: FuncType, func: F) -> Func
    where
        F: for<'a> Fn(
                Caller<'a, T>,
                &'a [Val],
                &'a mut [Val],
            ) -> Box<dyn Future<Output = crate::Result<()>> + Send + 'a>
            + Send
            + Sync
            + 'static,
        T: 'static,
    {
        todo!()
    }

    pub fn wrap<T, F, Params, Results>(store: &mut Store<T>, func: F) -> Func
    where
        F: for<'a> Fn(Caller<'a, T>, Params) -> Box<dyn Future<Output = Results> + Send + 'a>
            + Send
            + Sync
            + 'static,
        Params: WasmParams,
        Results: WasmResults,
        T: 'static,
    {
        todo!()
    }

    pub fn typed<Params, Results>(
        self,
        store: &StoreOpaque,
    ) -> crate::Result<TypedFunc<Params, Results>>
    where
        Params: WasmParams,
        Results: WasmResults,
    {
        todo!()
    }

    /// Calls the given function with the provided arguments and places the results in the provided
    /// results slice.
    pub async fn call<T: Send>(
        &self,
        _store: &mut Store<T>,
        _params: &[Val],
        _results: &mut [Val],
    ) -> crate::Result<()> {
        todo!()
    }

    /// Calls the given function with the provided arguments and places the results in the provided
    /// results slice.
    pub async unsafe fn call_unchecked(
        self,
        _store: Pin<&mut StoreOpaque>,
        _params: &[Val],
        _results: &mut [Val],
    ) -> crate::Result<()> {
        todo!()
    }

    pub fn ty(self, store: &StoreOpaque) -> FuncType {
        FuncType::from_shared_type_index(store.engine(), self.type_index(store))
    }

    pub fn matches_ty(self, store: &StoreOpaque, ty: FuncType) -> bool {
        let actual_ty = self.ty(store);
        actual_ty.matches(&ty)
    }

    pub(super) fn type_index(self, store: &StoreOpaque) -> VMSharedTypeIndex {
        // Safety: TODO
        unsafe { self.vm_func_ref(store).as_ref().type_index }
    }

    pub(crate) fn from_exported_function(
        store: Pin<&mut StoreOpaque>,
        export: ExportedFunction,
    ) -> Self {
        let stored = store.add_function(FuncData {
            kind: FuncKind::StoreOwned { export },
        });
        Self(stored)
    }

    pub(super) fn as_vmfunction_import(
        self,
        store: Pin<&mut StoreOpaque>,
        module: &Module,
    ) -> VMFunctionImport {
        let f = self.vm_func_ref(&*store);

        // Safety: TODO
        unsafe {
            VMFunctionImport {
                wasm_call: f.as_ref().wasm_call.unwrap_or_else(|| {
                    // Assert that this is a array-call function, since those
                    // are the only ones that could be missing a `wasm_call`
                    // trampoline.
                    let _ = VMArrayCallHostFuncContext::from_opaque(f.as_ref().vmctx.as_non_null());

                    let sig = self.type_index(&*store);

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

    pub(super) unsafe fn from_vm_func_ref(
        store: Pin<&mut StoreOpaque>,
        func_ref: NonNull<VMFuncRef>,
    ) -> Self {
        // Safety: ensured by caller
        unsafe {
            debug_assert!(func_ref.as_ref().type_index != VMSharedTypeIndex::default());
            Func::from_exported_function(store, ExportedFunction { func_ref })
        }
    }

    pub(super) fn vm_func_ref(self, store: &StoreOpaque) -> NonNull<VMFuncRef> {
        match &store.get_function(self.0).unwrap().kind {
            FuncKind::StoreOwned { export } => export.func_ref,
            FuncKind::SharedHost(func) => func.func_ref(),
            FuncKind::Host(func) => func.func_ref(),
        }
    }
}
