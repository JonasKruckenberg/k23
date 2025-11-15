// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod frame_alloc;

use core::mem;
use core::ops::Range;

pub use frame_alloc::BootstrapAllocator;

use crate::frame_alloc::{AllocError, FrameAllocator};
use crate::{
    AddressSpace, Arch, Flush, MemoryAttributes, MemoryMode, PhysicalAddress, Table,
    VirtualAddress, WriteOrExecute,
};

pub struct BootstrapArch<A: Arch> {
    inner: A,
}

impl<A: Arch> Arch for BootstrapArch<A> {
    type PageTableEntry = A::PageTableEntry;

    fn memory_mode(&self) -> &'static MemoryMode {
        self.inner.memory_mode()
    }

    fn active_table(&self) -> Option<PhysicalAddress> {
        self.inner.active_table()
    }

    unsafe fn set_active_table(&self, address: PhysicalAddress) {
        // Safety: ensured by the caller.
        unsafe { self.inner.set_active_table(address) }
    }

    fn fence(&self, range: Range<VirtualAddress>) {
        self.inner.fence(range);
    }

    fn fence_all(&self) {
        self.inner.fence_all();
    }

    fn phys_to_virt(&self, address: PhysicalAddress) -> VirtualAddress {
        // Safety: during bootstrap no address translation takes place meaning physical addresses *are*
        // virtual addresses.
        unsafe { mem::transmute(address) }
    }
}

impl<A: Arch> AddressSpace<BootstrapArch<A>> {
    /// Constructs **and bootstraps** a new AddressSpace with a freshly allocated root page table.
    ///
    /// # Errors
    ///
    /// Returns Err(AllocError) when allocating the root page table fails.
    pub fn new_bootstrap<R: lock_api::RawMutex>(
        inner: A,
        frame_allocator: &BootstrapAllocator<R>,
        flush: &mut Flush,
    ) -> Result<Self, AllocError> {
        AddressSpace::new(BootstrapArch { inner }, frame_allocator, flush)
    }

    /// Maps the physical memory region managed by the bootstrap allocator into the physmap region
    /// described by this architectures memory mode.
    ///
    /// If this returns `Ok`, the mapping is added to the address space.
    ///
    /// Note that this method **does not** establish any ordering between address space modification
    /// and accesses through the mapping, nor does it imply a page table cache flush. To ensure the
    /// new mapping is visible to the calling CPU you must call [`flush`][Flush::flush] on the returned `[Flush`].
    ///
    /// After the modifications have been synchronized with current execution, all accesses to the virtual
    /// address range will translate to accesses of the physical address range and adhere to the
    /// access rules established by the `MemoryAttributes`.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the mapping cannot be established and the address space remains
    /// unaltered.
    pub fn map_physical_memory<R: lock_api::RawMutex>(
        &mut self,
        frame_allocator: &BootstrapAllocator<R>,
        flush: &mut Flush,
    ) -> Result<(), AllocError> {
        for region in frame_allocator.regions() {
            let virt = Range {
                start: region
                    .start
                    .to_virt(self.arch().memory_mode().physmap_base()),
                end: region.end.to_virt(self.arch().memory_mode().physmap_base()),
            };

            // Safety: we just created the address space and `BootstrapAllocator` checks its regions to
            // not be overlapping (1.). It will also align regions to at least page size (2., 3.).
            unsafe {
                self.map_contiguous(
                    virt,
                    region.start,
                    MemoryAttributes::new()
                        .with(MemoryAttributes::READ, true)
                        .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write),
                    frame_allocator.by_ref(),
                    flush,
                )?;
            }
        }

        Ok(())
    }

    /// Identity-maps the physical address range with the specified memory attributes.
    ///
    /// If this returns `Ok`, the mapping is added to the address space.
    ///
    /// Note that this method **does not** establish any ordering between address space modification
    /// and accesses through the mapping, nor does it imply a page table cache flush. To ensure the
    /// new mapping is visible to the calling CPU you must call [`flush`][Flush::flush] on the returned `[Flush`].
    ///
    /// After the modifications have been synchronized with current execution, all accesses to the virtual
    /// address range will translate to accesses of the physical address range and adhere to the
    /// access rules established by the `MemoryAttributes`.
    ///
    /// # Safety
    ///
    /// 1. The entire virtual address range corresponding to `phys` must be unmapped.
    /// 2. `phys` must be aligned to `at least the smallest architecture block size.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the mapping cannot be established and the address space remains
    /// unaltered.
    pub unsafe fn map_identity<R: lock_api::RawMutex>(
        &mut self,
        phys: Range<PhysicalAddress>,
        attributes: MemoryAttributes,
        frame_allocator: &BootstrapAllocator<R>,
        flush: &mut Flush,
    ) -> Result<(), AllocError> {
        let virt = Range {
            start: self.arch().inner.phys_to_virt(phys.start),
            end: self.arch().inner.phys_to_virt(phys.end),
        };

        // Safety: ensured by caller.
        unsafe { self.map_contiguous(virt, phys.start, attributes, frame_allocator, flush) }
    }

    /// Finish the address space bootstrapping phase and activate the address space on this CPU (set
    /// this CPUs page table).
    ///
    /// # Safety
    ///
    /// After this method returns, all pointers become dangling and as such any access through
    /// pre-existing pointers is Undefined Behaviour. This includes implicit references by the CPU
    /// such as the instruction pointer.
    pub unsafe fn finish_bootstrap_and_activate(self) -> AddressSpace<A> {
        let (BootstrapArch { inner }, root_table) = self.into_raw_parts();
        let root_table = root_table.address();

        // Safety: ensured by caller
        unsafe {
            inner.set_active_table(root_table);
        }

        // Safety: We have retrieved the table address from and owned root table
        AddressSpace::from_raw_parts(inner, unsafe { Table::from_raw_parts(root_table, 0) })
    }
}
