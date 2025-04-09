// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::TrapKind;
use crate::wasm::vm::mmap_vec::MmapVec;
use crate::wasm::vm::provenance::VmPtr;
use crate::wasm::vm::{VMFuncRef, VMTableDefinition};
use anyhow::anyhow;
use core::ptr;
use core::ptr::NonNull;
use core::range::Range;

#[derive(Debug, Clone, Copy)]
pub enum TableElement {
    /// A `funcref`.
    FuncRef(Option<NonNull<VMFuncRef>>),
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum TableElementType {
    Func,
    GcRef,
}

#[derive(Debug)]
pub struct Table {
    /// The underlying allocation backing this memory
    elements: MmapVec<TableElement>,
    /// The current size of the table.
    size: usize,
    /// The optional maximum accessible size, in elements, for this table.
    maximum: Option<usize>,
}
unsafe impl Send for Table {}
unsafe impl Sync for Table {}

impl Table {
    pub(crate) unsafe fn from_parts(
        elements: MmapVec<TableElement>,
        maximum: Option<usize>,
    ) -> Self {
        Self {
            size: elements.len(),
            elements,
            maximum,
        }
    }

    pub fn size(&self) -> usize {
        self.elements.len()
    }

    pub fn init_func(
        &mut self,
        dst: usize,
        items: impl ExactSizeIterator<Item = Option<NonNull<VMFuncRef>>>,
    ) -> Result<(), TrapKind> {
        let dst = usize::try_from(dst).map_err(|_| TrapKind::TableOutOfBounds)?;
        let elements = self
            .elements
            .slice_mut()
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
        let end = start
            .checked_add(len)
            .ok_or_else(|| TrapKind::TableOutOfBounds)?;

        if end > self.size() {
            return Err(TrapKind::TableOutOfBounds);
        }

        self.elements.slice_mut()[start..end].fill(val);

        Ok(())
    }

    pub fn get(&self, index: u64) -> Option<TableElement> {
        let index = usize::try_from(index).ok()?;
        self.elements.get(index).cloned()
    }

    pub fn set(&mut self, index: u64, elem: TableElement) -> crate::Result<()> {
        let index: usize = index.try_into()?;
        let slot = self
            .elements
            .slice_mut()
            .get_mut(index)
            .ok_or(anyhow!("table element index out of bounds"))?;
        *slot = elem;

        Ok(())
    }

    pub fn grow(&mut self, delta: u64, init: TableElement) -> Result<Option<usize>, TrapKind> {
        let old_size = self.size();

        // Don't try to resize the table if its size isn't changing, just return
        // success.
        if delta == 0 {
            return Ok(Some(old_size));
        }

        let delta = usize::try_from(delta).map_err(|_| TrapKind::TableOutOfBounds)?;
        let new_size = old_size
            .checked_add(delta)
            .ok_or(TrapKind::TableOutOfBounds)?;

        // The WebAssembly spec requires failing a `table.grow` request if
        // it exceeds the declared limits of the table. We may have set lower
        // limits in the instance allocator as well.
        if let Some(max) = self.maximum {
            if new_size > max {
                return Ok(None);
            }
        }

        // we only support static tables that have all their memory reserved (not allocated) upfront
        // this means resizing is as simple as just updating the size field
        self.size = new_size;

        self.fill(
            u64::try_from(old_size).unwrap(),
            init,
            u64::try_from(delta).unwrap(),
        )
        .expect("table should not be out of bounds");

        Ok(Some(old_size))
    }

    pub fn copy(
        dst_table: *mut Self,
        src_table: *mut Self,
        dst_index: u64,
        src_index: u64,
        len: u64,
    ) -> Result<(), TrapKind> {
        unsafe {
            let src_index = usize::try_from(src_index).map_err(|_| TrapKind::TableOutOfBounds)?;
            let dst_index = usize::try_from(dst_index).map_err(|_| TrapKind::TableOutOfBounds)?;
            let len = usize::try_from(len).map_err(|_| TrapKind::TableOutOfBounds)?;

            if src_index
                .checked_add(len)
                .map_or(true, |n| n > (*src_table).size())
                || dst_index
                    .checked_add(len)
                    .map_or(true, |m| m > (*dst_table).size())
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

    fn copy_elements_within(&mut self, dst_range: Range<usize>, src_range: Range<usize>) {
        self.elements
            .slice_mut()
            .copy_within(src_range, dst_range.start);
    }

    fn copy_elements(
        dst_table: &mut Self,
        src_table: &Self,
        dst_range: Range<usize>,
        src_range: Range<usize>,
    ) {
        // This can only be used when copying between different tables
        debug_assert!(!ptr::eq(dst_table, src_table));

        dst_table.elements.slice_mut()[dst_range]
            .copy_from_slice(&src_table.elements.slice()[src_range]);
    }

    pub fn as_vmtable_definition(&self) -> VMTableDefinition {
        unsafe {
            VMTableDefinition {
                base: VmPtr::from(NonNull::new_unchecked(
                    self.elements.as_ptr().cast_mut().cast(),
                )),
                current_elements: self.elements.len().into(),
            }
        }
    }
}
