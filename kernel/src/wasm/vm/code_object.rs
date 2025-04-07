// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::mem::{AddressSpace, Mmap, VirtualAddress};
use crate::wasm::compile::FunctionLoc;
use crate::wasm::vm::MmapVec;
use crate::wasm::Trap;
use alloc::vec;
use alloc::vec::Vec;
use anyhow::Context;
use core::range::Range;
use core::slice;

#[derive(Debug)]
pub struct CodeObject {
    mmap: Mmap,
    len: usize,
    published: bool,

    trap_offsets: Vec<u32>,
    traps: Vec<Trap>,
}

impl CodeObject {
    pub const fn empty() -> Self {
        Self {
            mmap: Mmap::new_empty(),
            len: 0,
            published: false,
            trap_offsets: vec![],
            traps: vec![],
        }
    }

    pub fn new(mmap_vec: MmapVec<u8>, trap_offsets: Vec<u32>, traps: Vec<Trap>) -> Self {
        let (mmap, size) = mmap_vec.into_parts();
        Self {
            mmap,
            len: size,
            published: false,
            trap_offsets,
            traps,
        }
    }

    pub fn publish(&mut self, aspace: &mut AddressSpace) -> crate::Result<()> {
        debug_assert!(!self.published);
        self.published = true;

        if self.mmap.is_empty() {
            tracing::warn!("Compiled module has no code to publish");
            return Ok(());
        }

        // Switch the executable portion from readonly to read/execute.
        self.mmap
            .make_executable(aspace, true)
            .context("Failed to mark mmap'ed region as executable")?;

        Ok(())
    }

    pub fn text(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.mmap.as_ptr(), self.len) }
    }

    #[inline]
    pub fn text_range(&self) -> Range<VirtualAddress> {
        let start = self.mmap.range().start;

        Range::from(start..start.checked_add(self.len).unwrap())
    }

    pub fn resolve_function_loc(&self, func_loc: FunctionLoc) -> usize {
        let text_range = {
            let r = self.text_range();
            r.start.get()..r.end.get()
        };

        let addr = text_range.start + func_loc.start as usize;

        tracing::trace!(
            "resolve_function_loc {func_loc:?}, text {:?} => {:?}",
            self.mmap.as_ptr(),
            addr,
        );

        // Assert the function location actually lies in our text section
        debug_assert!(
            text_range.start <= addr
                && text_range.end >= addr.saturating_add(usize::try_from(func_loc.length).unwrap())
        );

        addr
    }

    pub fn lookup_trap_code(&self, text_offset: usize) -> Option<Trap> {
        let text_offset = u32::try_from(text_offset).unwrap();

        let index = self
            .trap_offsets
            .binary_search_by_key(&text_offset, |val| *val)
            .ok()?;

        Some(self.traps[index])
    }
}
