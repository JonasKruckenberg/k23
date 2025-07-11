// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::pin::Pin;

use crate::TagType;
use crate::store::{StoreOpaque, Stored};
use crate::vm::{ExportedTag, VMTagImport};

#[derive(Clone, Copy, Debug)]
pub struct Tag(Stored<ExportedTag>);

impl Tag {
    pub fn ty(self, store: &StoreOpaque) -> TagType {
        todo!()
    }

    pub(crate) fn from_exported_tag(store: Pin<&mut StoreOpaque>, export: ExportedTag) -> Self {
        let stored = store.add_tag(export);
        Self(stored)
    }

    pub(crate) fn as_vmtag_import(&self, store: Pin<&mut StoreOpaque>) -> VMTagImport {
        todo!()
    }
}
