use crate::vm::{AddressSpace, UserMmap};
use crate::wasm::runtime::VMMemoryDefinition;
use crate::wasm::translate::MemoryDesc;
use crate::wasm::utils::round_usize_up_to_host_pages;
use crate::wasm::{Error, MEMORY_MAX};
use core::range::Range;

#[derive(Debug)]
pub struct Memory {
    /// The underlying allocation backing this memory
    mmap: UserMmap,
    /// The current length of this Wasm memory, in bytes.
    len: usize,
    /// The optional maximum accessible size, in bytes, for this linear memory.
    ///
    /// This **does not** include guard pages and might be smaller than `self.accessible`
    /// since the underlying allocation is always a multiple of the host page size.
    maximum: Option<usize>,
    /// The log2 of this Wasm memory's page size, in bytes.
    page_size_log2: u8,
    /// Size in bytes of extra guard pages after the end to
    /// optimize loads and stores with constant offsets.
    offset_guard_size: usize,
}

impl Memory {
    #[expect(clippy::unnecessary_wraps, reason = "TODO")]
    pub fn try_new(
        _aspace: &mut AddressSpace,
        desc: &MemoryDesc,
        actual_minimum_bytes: usize,
        actual_maximum_bytes: Option<usize>,
    ) -> crate::wasm::Result<Self> {
        let offset_guard_bytes = usize::try_from(desc.offset_guard_size).unwrap();
        // Ensure that our guard regions are multiples of the host page size.
        let offset_guard_bytes = round_usize_up_to_host_pages(offset_guard_bytes);

        // let bound_bytes = round_usize_up_to_host_pages(MEMORY_MAX);
        // let allocation_bytes = bound_bytes.min(actual_maximum_bytes.unwrap_or(usize::MAX));
        // let request_bytes = allocation_bytes + offset_guard_bytes;

        // let mmap = UserMmap::new_zeroed(aspace, request_bytes, 2 * 1048576).map_err(|_| Error::MmapFailed)?;

        Ok(Self {
            mmap: UserMmap::new_empty(),
            len: actual_minimum_bytes,
            maximum: actual_maximum_bytes,
            page_size_log2: desc.page_size_log2,
            offset_guard_size: offset_guard_bytes,
        })
    }

    pub fn with_user_slice_mut<F>(&mut self, aspace: &mut AddressSpace, range: Range<usize>, f: F)
    where
        F: FnOnce(&mut [u8]),
    {
        self.mmap.with_user_slice_mut(aspace, range, f).unwrap();
    }

    pub(crate) fn as_vmmemory_definition(&mut self) -> VMMemoryDefinition {
        VMMemoryDefinition {
            base: self.mmap.as_mut_ptr(),
            current_length: self.len.into(),
        }
    }
}
