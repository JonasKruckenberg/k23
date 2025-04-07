// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::store::{StoreOpaque, Stored};
use crate::wasm::types::GlobalType;
use crate::wasm::values::Val;
use crate::wasm::vm::{ExportedGlobal, VMGlobalImport};

#[derive(Clone, Copy, Debug)]
pub struct Global(Stored<ExportedGlobal>);

impl Global {
    pub fn new(store: &mut StoreOpaque, ty: GlobalType, val: Val) -> crate::Result<Self> {
        todo!()
    }

    pub fn ty(&self, store: &StoreOpaque) -> GlobalType {
        todo!()
    }

    pub fn get(&self, store: &StoreOpaque) -> Val {
        todo!()
    }

    pub fn set(&self, store: &mut StoreOpaque, val: Val) -> crate::Result<()> {
        todo!()
    }

    pub(super) fn from_exported_global(store: &mut StoreOpaque, export: ExportedGlobal) -> Self {
        todo!()
    }
    pub(super) fn as_vmglobal_import(&self, store: &mut StoreOpaque) -> VMGlobalImport {
        todo!()
    }
}
