// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::vm::mmap_vec::MmapVec;
use crate::wasm::vm::provenance::VmPtr;
use crate::wasm::vm::{VMFuncRef, VMTableDefinition};
use crate::wasm::TrapKind;
use core::ptr::NonNull;

pub enum TableElement {
    /// A `funcref`.
    FuncRef(Option<NonNull<VMFuncRef>>),

    // /// A GC reference.
    // GcRef(Option<VMGcRef>),
    /// An uninitialized funcref value. This should never be exposed
    /// beyond the `wasmtime` crate boundary; the upper-level code
    /// (which has access to the info needed for lazy initialization)
    /// will replace it when fetched.
    UninitFunc,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum TableElementType {
    Func,
    GcRef,
}

#[derive(Debug)]
pub struct Table {
    /// The underlying allocation backing this memory
    elements: MmapVec<Option<NonNull<TableElement>>>,
    /// The optional maximum accessible size, in elements, for this table.
    maximum: Option<usize>,
}

impl Table {
    pub(crate) unsafe fn from_parts(
        elements: MmapVec<Option<NonNull<TableElement>>>,
        maximum: Option<usize>,
    ) -> Self {
        Self { elements, maximum }
    }

    pub fn init_func(
        &self,
        _start: usize,
        _elements: impl Iterator<Item = Option<NonNull<VMFuncRef>>>,
    ) -> Result<(), TrapKind> {
        todo!()
    }
    pub fn size(&self) -> usize {
        todo!()
    }
    pub fn as_vmtable_definition(&mut self) -> VMTableDefinition {
        unsafe {
            VMTableDefinition {
                base: VmPtr::from(NonNull::new_unchecked(self.elements.as_mut_ptr().cast())),
                current_elements: self.elements.len().into(),
            }
        }
    }
}
