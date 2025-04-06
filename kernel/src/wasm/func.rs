// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::indices::VMSharedTypeIndex;
use crate::wasm::store::{StoreOpaque, Stored};
use crate::wasm::vm::{ExportedFunction, VMFuncRef, VMFunctionImport};
use core::ffi::c_void;
use core::ptr::NonNull;
use crate::wasm::types::FuncType;

#[derive(Clone, Copy, Debug)]
pub struct Func(Stored<FuncData>);
pub struct FuncData {}

impl Func {
    pub async fn call(&self) {}

    pub fn ty(&self, store: &StoreOpaque) -> FuncType {
        todo!()
    }

    pub fn matches_ty(&self, store: &StoreOpaque, ty: FuncType) -> bool {
        todo!()
    }

    pub(super) fn as_vmfunction_import(&self, store: &mut StoreOpaque) -> VMFunctionImport {
        // unsafe {
        //     let f = {
        //         let func_data = &mut store.store_data_mut()[self.0];
        //         // If we already patched this `funcref.wasm_call` and saved a
        //         // copy in the store, use the patched version. Otherwise, use
        //         // the potentially un-patched version.
        //         if let Some(func_ref) = func_data.in_store_func_ref {
        //             func_ref.as_non_null()
        //         } else {
        //             func_data.export().func_ref
        //         }
        //     };
        //     VMFunctionImport {
        //         wasm_call: if let Some(wasm_call) = f.as_ref().wasm_call {
        //             wasm_call.into()
        //         } else {
        //             // Assert that this is a array-call function, since those
        //             // are the only ones that could be missing a `wasm_call`
        //             // trampoline.
        //             let _ = VMArrayCallHostFuncContext::from_opaque(f.as_ref().vmctx.as_non_null());
        //
        //             let sig = self.type_index(store.store_data());
        //             module.wasm_to_array_trampoline(sig).expect(
        //                 "if the wasm is importing a function of a given type, it must have the \
        //                  type's trampoline",
        //             ).into()
        //         },
        //         array_call: f.as_ref().array_call,
        //         vmctx: f.as_ref().vmctx,
        //     }
        // }

        todo!()
    }

    pub(super) fn comes_from_same_store(&self, store: &StoreOpaque) -> bool {
        store.has_function(self.0)
    }

    pub(super) unsafe fn from_exported_function(
        store: &mut StoreOpaque,
        export: ExportedFunction,
    ) -> Self {
        todo!()
    }

    pub(super) unsafe fn from_vm_func_ref(
        store: &mut StoreOpaque,
        func_ref: NonNull<VMFuncRef>,
    ) -> Self {
        debug_assert!(func_ref.as_ref().type_index != VMSharedTypeIndex::default());
        Func::from_exported_function(store, ExportedFunction { func_ref })
    }

    pub(super) fn vm_func_ref(&self, store: &mut StoreOpaque) -> NonNull<VMFuncRef> {
        todo!()
    }

    pub(super) unsafe fn from_vmval(store: &mut StoreOpaque, raw: *mut c_void) -> Option<Self> {
        Some(Func::from_vm_func_ref(store, NonNull::new(raw.cast())?))
    }

    /// Extracts the raw value of this `Func`, which is owned by `store`.
    ///
    /// This function returns a value that's suitable for writing into the
    /// `funcref` field of the [`ValRaw`] structure.
    ///
    /// # Unsafety
    ///
    /// The returned value is only valid for as long as the store is alive and
    /// this function is properly rooted within it. Additionally this function
    /// should not be liberally used since it's a very low-level knob.
    pub(super) unsafe fn to_vmval(&self, mut store: &mut StoreOpaque) -> *mut c_void {
        self.vm_func_ref(store).as_ptr().cast()
    }
}
