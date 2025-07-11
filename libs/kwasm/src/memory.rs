// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
use core::pin::Pin;

use crate::store::{StoreOpaque, Stored};
use crate::vm::{ExportedMemory, VMMemoryImport};
use crate::{MemoryType, Store};

/// A WebAssembly linear memory.
#[derive(Clone, Copy, Debug)]
pub struct Memory(Stored<ExportedMemory>);

/// Error for out of bounds [`Memory`] access.
#[derive(Debug)]
#[non_exhaustive]
pub struct MemoryAccessError {
    // Keep struct internals private for future extensibility.
    _private: (),
}

// ===== impl Memory =====

impl Memory {
    pub fn data<'a, T: 'static>(&self, store: &'a Store<T>) -> &'a [u8] {
        todo!()
    }

    pub fn data_mut<'a, T: 'static>(&self, store: &'a mut Store<T>) -> &'a mut [u8] {
        todo!()
    }

    pub fn data_and_store_mut<'a, T: 'static>(
        &self,
        store: &'a mut Store<T>,
    ) -> (&'a mut [u8], &'a mut T) {
        todo!()
    }

    pub fn size(&self, store: &StoreOpaque) -> u64 {
        todo!()
    }

    pub fn page_size(&self, store: &StoreOpaque) -> u64 {
        todo!()
    }

    pub fn page_size_log2(&self, store: &StoreOpaque) -> u8 {
        todo!()
    }

    pub fn grow(&self, store: Pin<&mut StoreOpaque>, delta: u64) -> crate::Result<u64> {
        todo!()
    }

    pub fn read(
        &self,
        store: &StoreOpaque,
        offset: usize,
        buffer: &mut [u8],
    ) -> Result<(), MemoryAccessError> {
        todo!()
    }

    pub fn write(
        &self,
        store: Pin<&mut StoreOpaque>,
        offset: usize,
        buffer: &[u8],
    ) -> Result<(), MemoryAccessError> {
        todo!()
    }

    pub fn ty(&self, store: &StoreOpaque) -> MemoryType {
        todo!()
    }

    pub(crate) fn from_exported_memory(
        store: Pin<&mut StoreOpaque>,
        export: ExportedMemory,
    ) -> Self {
        let stored = store.add_memory(export);
        Self(stored)
    }

    pub(crate) fn as_vmmemory_import(&self, store: Pin<&mut StoreOpaque>) -> VMMemoryImport {
        todo!()
    }
}

// ===== impl MemoryAccessError =====

impl fmt::Display for MemoryAccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "out of bounds memory access")
    }
}

impl core::error::Error for MemoryAccessError {}
