// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::vec;
use alloc::vec::Vec;
use core::ops::Range;
use core::ptr::NonNull;
use core::slice;

use anyhow::Context;
use cranelift_entity::PrimaryMap;
use kmem::VirtualAddress;

use crate::mem::{AddressSpace, Mmap};
use crate::wasm::TrapKind;
use crate::wasm::compile::{CompiledFunctionInfo, FunctionLoc};
use crate::wasm::indices::{DefinedFuncIndex, ModuleInternedTypeIndex};
use crate::wasm::vm::{MmapVec, VMWasmCallFunction};

#[derive(Debug)]
pub struct CodeObject {
    mmap: Mmap,
    len: usize,
    published: bool,

    trap_offsets: Vec<u32>,
    traps: Vec<TrapKind>,
    wasm_to_host_trampolines: Vec<(ModuleInternedTypeIndex, FunctionLoc)>,
    function_info: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
}

impl CodeObject {
    pub fn empty() -> Self {
        Self {
            mmap: Mmap::new_empty(),
            len: 0,
            published: false,
            trap_offsets: vec![],
            traps: vec![],
            wasm_to_host_trampolines: vec![],
            function_info: PrimaryMap::new(),
        }
    }

    pub fn new(
        mmap_vec: MmapVec<u8>,
        trap_offsets: Vec<u32>,
        traps: Vec<TrapKind>,
        wasm_to_host_trampolines: Vec<(ModuleInternedTypeIndex, FunctionLoc)>,
        function_info: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
    ) -> Self {
        let (mmap, size) = mmap_vec.into_parts();
        Self {
            mmap,
            len: size,
            published: false,
            trap_offsets,
            traps,
            wasm_to_host_trampolines,
            function_info,
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
        let base = self.mmap.as_ptr();
        if base.is_null() {
            &[]
        } else {
            // Safety: we have checked the slice is valid (both above and through construction)
            unsafe { slice::from_raw_parts(self.mmap.as_ptr(), self.len) }
        }
    }

    #[inline]
    pub fn text_range(&self) -> Range<VirtualAddress> {
        let start = self.mmap.range().start;

        start..start.add(self.len)
    }

    pub fn resolve_function_loc(&self, func_loc: FunctionLoc) -> usize {
        let text_range = self.text_range();
        let addr = text_range.start.get() + func_loc.start as usize;

        tracing::trace!(
            "resolve_function_loc {func_loc:?}, text {:?} => {:?}",
            self.mmap.as_ptr(),
            addr,
        );

        // Assert the function location actually lies in our text section
        debug_assert!(
            text_range.start.get() <= addr
                && text_range.end.get()
                    >= addr.saturating_add(usize::try_from(func_loc.length).unwrap())
        );

        addr
    }

    pub fn lookup_trap_code(&self, text_offset: usize) -> Option<TrapKind> {
        let text_offset = u32::try_from(text_offset).unwrap();

        let index = self
            .trap_offsets
            .binary_search_by_key(&text_offset, |val| *val)
            .ok()?;

        Some(self.traps[index])
    }

    pub(crate) fn function_info(&self) -> &PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo> {
        &self.function_info
    }

    pub fn wasm_to_host_trampoline(
        &self,
        sig: ModuleInternedTypeIndex,
    ) -> NonNull<VMWasmCallFunction> {
        let Ok(idx) = self
            .wasm_to_host_trampolines
            .binary_search_by_key(&sig, |entry| entry.0)
        else {
            panic!("missing trampoline for {sig:?}")
        };

        let (_, loc) = self.wasm_to_host_trampolines[idx];

        NonNull::new(self.resolve_function_loc(loc) as *mut VMWasmCallFunction).unwrap()
    }
}
