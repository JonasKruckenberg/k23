use crate::arch::EntryFlags;
use crate::frame_alloc::with_frame_alloc;
use crate::kconfig;
use alloc::sync::Arc;
use core::alloc::{AllocError, Allocator, Layout};
use core::fmt;
use core::fmt::Formatter;
use core::ops::Range;
use core::ptr::NonNull;
use kstd::sync::Mutex;
use linked_list_allocator::Heap;
use vmm::{AddressRangeExt, Flush, Mapper, Mode, VirtualAddress};

/// A type that knows how to allocate and deallocate memory in userspace.
///
/// We quite often need to allocate things in userspace: compiled module images, stacks,
/// tables, memories, VMContexts, ... essentially everything that is *not* privileged data
/// owned and managed by the kernel is allocated through this.
///
/// This type is used from within a [`Store`] to manage its memory.
///
/// # Address Spaces
///
/// In classical operating systems, a fresh virtual memory address space is allocated to each process
/// for isolation reasons. Conceptually a process owns its allocated address space and backing memory
/// (once a process gets dropped, its resources get freed).
///
/// WebAssembly defines a separate "Data Owner" entity (the [`Store`]) to which all data of one or more
/// instances belongs. So even though a WebAssembly [`Instance`] most closely resembles a process, it is
/// the `Store` that owns the allocated address space and backing memory.
///
/// In k23 we allocate one address space per [`Store`] which turns out to the be same thing as classical
/// operating systems in practice, since each user program gets run in its own [`Store`] .
/// But this approach allows us more flexibility in how we manage memory: E.g. we can have process groups
/// that share a common [`Store`] and can therefore share resources much more efficiently.
#[derive(Debug, Clone)]
pub struct GuestAllocator(Arc<Mutex<GuestAllocatorInner>>);

pub struct GuestAllocatorInner {
    asid: usize,
    root_table: VirtualAddress,
    virt_offset: VirtualAddress,
    // we don't have many allocations, just a few large chunks (e.g. CodeMemory, Stack, Memories)
    // so a simple linked list should suffice.
    // TODO measure and verify this assumption
    inner: Heap,
}

impl GuestAllocator {
    pub unsafe fn new_in_kernel_space(virt_offset: VirtualAddress) -> Result<Self, AllocError> {
        let root_table = kconfig::MEMORY_MODE::get_active_table(0);

        let mut inner = GuestAllocatorInner {
            root_table: kconfig::MEMORY_MODE::phys_to_virt(root_table),
            asid: 0,
            inner: Heap::empty(),
            virt_offset,
        };

        let (mem_virt, flush) = inner.map_additional_pages(16)?;
        flush.flush().unwrap();

        unsafe {
            inner
                .inner
                .init(mem_virt.start.as_raw() as *mut u8, mem_virt.size());
        }

        Ok(Self(Arc::new(Mutex::new(inner))))
    }

    pub fn asid(&self) -> usize {
        self.0.lock().asid
    }

    pub fn root_table(&self) -> VirtualAddress {
        self.0.lock().root_table
    }
}

unsafe impl Allocator for GuestAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let mut inner = self.0.lock();

        let ptr = if let Ok(ptr) = inner.inner.allocate_first_fit(layout) {
            ptr
        } else {
            let grow_by = layout.size() - inner.inner.free();
            let grow_by_pages = grow_by.div_ceil(kconfig::PAGE_SIZE);
            log::debug!("growing guest alloc by {grow_by_pages} pages");

            // TODO probably amortize growth
            let (_, flush) = inner.map_additional_pages(grow_by_pages)?;
            flush.flush().unwrap();

            unsafe { inner.inner.extend(grow_by_pages * kconfig::PAGE_SIZE) };

            // at this point the method below is guaranteed to not fail
            inner.inner.allocate_first_fit(layout).unwrap()
        };

        log::trace!("allocation request {ptr:?} {layout:?}");

        Ok(NonNull::slice_from_raw_parts(ptr, layout.size()))
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        log::trace!("deallocation request {ptr:?} {layout:?}");
        // TODO unmap pages

        let mut inner = self.0.lock();
        inner.inner.deallocate(ptr, layout)
    }
}

impl GuestAllocatorInner {
    fn map_additional_pages(
        &mut self,
        num_pages: usize,
    ) -> Result<(Range<VirtualAddress>, Flush<kconfig::MEMORY_MODE>), AllocError> {
        with_frame_alloc(|frame_alloc| {
            let mut mapper = Mapper::from_address(self.asid, self.root_table, frame_alloc);
            let mut flush = Flush::empty(self.asid);

            let mem_phys = {
                let start = mapper
                    .allocator_mut()
                    .allocate_frames_zeroed(num_pages)
                    .map_err(|_| AllocError)?;
                start..start.add(num_pages * kconfig::PAGE_SIZE)
            };

            let mem_virt = self.virt_offset..self.virt_offset.add(num_pages * kconfig::PAGE_SIZE);
            self.virt_offset = mem_virt.end;

            mapper
                .map_range(
                    mem_virt.clone(),
                    mem_phys,
                    EntryFlags::READ | EntryFlags::WRITE,
                    &mut flush,
                )
                .map_err(|_| AllocError)?;

            Ok((mem_virt, flush))
        })
    }
}

impl fmt::Debug for GuestAllocatorInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuestAllocatorInner")
            .field("asid", &self.asid)
            .field("root_table", &self.root_table)
            .field("virt_offset", &self.virt_offset)
            .finish()
    }
}
