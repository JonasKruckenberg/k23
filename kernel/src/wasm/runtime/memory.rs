use crate::wasm::runtime::VMMemoryDefinition;
use crate::wasm::translate::MemoryDesc;
use crate::wasm::utils::round_usize_up_to_host_pages;
use crate::wasm::MEMORY_MAX;

#[derive(Debug)]
pub struct Memory {
    /// The underlying allocation backing this memory
    mmap: Mmap,
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
    pub fn try_new(
        desc: &MemoryDesc,
        actual_minimum_bytes: usize,
        actual_maximum_bytes: Option<usize>,
    ) -> crate::wasm::Result<Self> {
        let offset_guard_bytes = usize::try_from(desc.offset_guard_size).unwrap();
        // Ensure that our guard regions are multiples of the host page size.
        let offset_guard_bytes = round_usize_up_to_host_pages(offset_guard_bytes);

        let bound_bytes = round_usize_up_to_host_pages(MEMORY_MAX);
        let allocation_bytes = bound_bytes.min(actual_maximum_bytes.unwrap_or(usize::MAX));

        let request_bytes = allocation_bytes + offset_guard_bytes;
        let mut mmap = Mmap::with_reserve(request_bytes)?;

        if actual_minimum_bytes > 0 {
            let accessible = round_usize_up_to_host_pages(actual_minimum_bytes);
            mmap.make_accessible(0, accessible)?;
        }

        Ok(Self {
            mmap,
            len: actual_minimum_bytes,
            maximum: actual_maximum_bytes,
            page_size_log2: desc.page_size_log2,
            offset_guard_size: offset_guard_bytes,
        })
    }

    pub(crate) fn as_slice_mut(&mut self) -> &mut [u8] {
        // Safety: The constructor has to ensure that `self.len` is valid.
        unsafe { self.mmap.slice_mut(0..self.len) }
    }

    pub(crate) fn as_vmmemory_definition(&mut self) -> VMMemoryDefinition {
        VMMemoryDefinition {
            base: self.mmap.as_mut_ptr(),
            current_length: self.len.into(),
        }
    }
}
