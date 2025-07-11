// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::vec::Vec;
use core::ptr;
use core::ptr::NonNull;
use core::range::Range;

use anyhow::anyhow;

use crate::TrapKind;
use crate::vm::provenance::VmPtr;
use crate::vm::{VMFuncRef, VMTableDefinition};

/// A WebAssembly table instance.
///
/// https://webassembly.github.io/spec/core/exec/runtime.html#table-instances
#[derive(Debug)]
pub struct Table {
    /// The underlying allocation backing this table
    mem: NonNull<[TableElement]>,
    /// The optional maximum accessible size, in elements, for this table.
    maximum: Option<usize>,
}

#[derive(Debug, Copy, Clone)]
pub enum TableElement {
    FuncRef(Option<NonNull<VMFuncRef>>),
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum TableElementType {
    Func,
    GcRef,
}

// === impl Table ===

impl Table {
    pub(super) fn new(mem: NonNull<[TableElement]>, maximum: Option<usize>) -> Self {
        Self { mem, maximum }
    }

    pub fn size(&self) -> usize {
        self.mem.len()
    }

    pub fn init_func(
        &mut self,
        dst: u64,
        items: impl ExactSizeIterator<Item = Option<NonNull<VMFuncRef>>>,
    ) -> Result<(), TrapKind> {
        let dst = usize::try_from(dst).map_err(|_| TrapKind::TableOutOfBounds)?;
        let elements = self
            .elements_mut()
            .get_mut(dst..)
            .and_then(|s| s.get_mut(..items.len()))
            .ok_or(TrapKind::TableOutOfBounds)?;

        for (item, slot) in items.zip(elements) {
            *slot = TableElement::FuncRef(item);
        }

        Ok(())
    }

    pub fn fill(&mut self, dst: u64, val: TableElement, len: u64) -> Result<(), TrapKind> {
        let start = usize::try_from(dst).map_err(|_| TrapKind::TableOutOfBounds)?;
        let len = usize::try_from(len).map_err(|_| TrapKind::TableOutOfBounds)?;
        let end = start.checked_add(len).ok_or(TrapKind::TableOutOfBounds)?;

        if end > self.size() {
            return Err(TrapKind::TableOutOfBounds);
        }

        self.elements_mut()[start..end].fill(val);

        Ok(())
    }

    pub fn get(&self, index: u64) -> Option<TableElement> {
        let index = usize::try_from(index).ok()?;
        self.elements().get(index).copied()
    }

    pub fn set(&mut self, index: u64, elem: TableElement) -> crate::Result<()> {
        let index: usize = index.try_into()?;
        let slot = self
            .elements_mut()
            .get_mut(index)
            .ok_or(anyhow!("table element index out of bounds"))?;
        *slot = elem;

        Ok(())
    }

    pub fn grow(&mut self, delta: u64, init: TableElement) -> Result<Option<usize>, TrapKind> {
        // let old_size = self.size();
        //
        // // Don't try to resize the table if its size isn't changing, just return
        // // success.
        // if delta == 0 {
        //     return Ok(Some(old_size));
        // }
        //
        // let delta = usize::try_from(delta).map_err(|_| TrapKind::TableOutOfBounds)?;
        // let new_size = old_size
        //     .checked_add(delta)
        //     .ok_or(TrapKind::TableOutOfBounds)?;
        //
        // // The WebAssembly spec requires failing a `table.grow` request if
        // // it exceeds the declared limits of the table. We may have set lower
        // // limits in the instance allocator as well.
        // if let Some(max) = self.maximum
        //     && new_size > max
        // {
        //     return Ok(None);
        // }
        //
        // self.elements.resize(new_size, init);
        //
        // Ok(Some(old_size))

        todo!()
    }

    pub fn copy(
        dst_table: *mut Self,
        src_table: *mut Self,
        dst_index: u64,
        src_index: u64,
        len: u64,
    ) -> Result<(), TrapKind> {
        // Safety: the table pointers are valid
        unsafe {
            let src_index = usize::try_from(src_index).map_err(|_| TrapKind::TableOutOfBounds)?;
            let dst_index = usize::try_from(dst_index).map_err(|_| TrapKind::TableOutOfBounds)?;
            let len = usize::try_from(len).map_err(|_| TrapKind::TableOutOfBounds)?;

            if src_index
                .checked_add(len)
                .is_none_or(|n| n > (*src_table).size())
                || dst_index
                    .checked_add(len)
                    .is_none_or(|m| m > (*dst_table).size())
            {
                return Err(TrapKind::TableOutOfBounds);
            }

            let src_range = Range::from(src_index..src_index + len);
            let dst_range = Range::from(dst_index..dst_index + len);

            if ptr::eq(dst_table, src_table) {
                (*dst_table).copy_elements_within(dst_range, src_range);
            } else {
                Self::copy_elements(&mut *dst_table, &*src_table, dst_range, src_range);
            }

            Ok(())
        }
    }

    fn elements(&self) -> &[TableElement] {
        unsafe { self.mem.as_ref() }
    }

    fn elements_mut(&mut self) -> &mut [TableElement] {
        unsafe { self.mem.as_mut() }
    }

    fn copy_elements_within(&mut self, dst_range: Range<usize>, src_range: Range<usize>) {
        self.elements_mut().copy_within(src_range, dst_range.start);
    }

    fn copy_elements(
        dst_table: &mut Self,
        src_table: &Self,
        dst_range: Range<usize>,
        src_range: Range<usize>,
    ) {
        // This can only be used when copying between different tables
        debug_assert!(!ptr::eq(dst_table, src_table));

        dst_table.elements_mut()[dst_range].copy_from_slice(&src_table.elements()[src_range]);
    }

    pub fn as_vmtable_definition(&self) -> VMTableDefinition {
        VMTableDefinition {
            base: VmPtr::from(NonNull::new(self.mem.as_ptr().cast()).unwrap()),
            current_elements: self.mem.len().into(),
        }
    }
}
