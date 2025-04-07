// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::store::{StoreOpaque, Stored};
use crate::wasm::vm::{ExportedTag, VMTagImport};

#[derive(Clone, Copy, Debug)]
pub struct Tag(Stored<ExportedTag>);

impl Tag {
    pub(super) fn from_exported_tag(store: &mut StoreOpaque, export: ExportedTag) -> Self {
        todo!()
    }
    pub(super) fn as_vmtag_import(&self, store: &mut StoreOpaque) -> VMTagImport {
        todo!()
    }
}
