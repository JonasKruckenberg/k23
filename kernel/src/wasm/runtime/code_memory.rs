use crate::vm::{AddressRangeExt, AddressSpace, UserMmap, VirtualAddress};
use crate::wasm::compile::{CompiledFunctionInfo, FunctionLoc};
use crate::wasm::indices::{DefinedFuncIndex, ModuleInternedTypeIndex};
use crate::wasm::runtime::{MmapVec, VMWasmCallFunction};
use crate::wasm::trap::Trap;
use crate::wasm::Error;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::range::Range;
use cranelift_entity::PrimaryMap;

#[derive(Debug)]
pub struct CodeMemory {
    mmap: UserMmap,
    len: usize,
    published: bool,

    trap_offsets: Vec<u32>,
    traps: Vec<Trap>,
    wasm_to_host_trampolines: Vec<(ModuleInternedTypeIndex, FunctionLoc)>,
    function_info: PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo>,
}

impl CodeMemory {
    pub fn new(
        mmap_vec: MmapVec<u8>,
        trap_offsets: Vec<u32>,
        traps: Vec<Trap>,
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

    pub fn publish(&mut self, aspace: &mut AddressSpace) -> crate::wasm::Result<()> {
        debug_assert!(!self.published);
        self.published = true;

        if self.mmap.is_empty() {
            tracing::warn!("Compiled module has no code to publish");
            return Ok(());
        }

        // Switch the executable portion from readonly to read/execute.
        self.mmap
            .make_executable(aspace, true)
            .map_err(|_| Error::MmapFailed)?;

        Ok(())
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

    pub(crate) fn function_info(&self) -> &PrimaryMap<DefinedFuncIndex, CompiledFunctionInfo> {
        &self.function_info
    }
    
    pub fn wasm_to_host_trampoline(&self, sig: ModuleInternedTypeIndex) -> *const VMWasmCallFunction {
        let idx = match self
            .wasm_to_host_trampolines
            .binary_search_by_key(&sig, |entry| entry.0)
        {
            Ok(idx) => idx,
            Err(_) => panic!("missing trampoline for {sig:?}"),
        };

        let (_, loc) = self.wasm_to_host_trampolines[idx];

        self.resolve_function_loc(loc) as *const VMWasmCallFunction
    }
}
