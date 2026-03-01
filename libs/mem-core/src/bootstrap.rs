use core::range::Range;

use crate::{
    AllocError, Arch, Flush, FrameAllocator, HardwareAddressSpace, MemoryAttributes, PhysMap,
    PhysicalAddress, VirtualAddress, WriteOrExecute,
};

pub struct Bootstrap<A> {
    pub(crate) address_space: A,
}

impl<A: Arch> Bootstrap<HardwareAddressSpace<A>> {
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
    pub unsafe fn map_identity(
        &mut self,
        phys: Range<PhysicalAddress>,
        attributes: MemoryAttributes,
        frame_allocator: impl FrameAllocator,
        physmap: &PhysMap,
    ) -> Result<(), AllocError> {
        let virt = Range {
            start: VirtualAddress::new(phys.start.get()),
            end: VirtualAddress::new(phys.end.get()),
        };

        let mut flush = Flush::new();

        // Safety: ensured by caller.
        unsafe {
            self.address_space.map_contiguous(
                virt,
                phys.start,
                attributes,
                frame_allocator,
                physmap,
                &mut flush,
            )?;
        }

        // Safety: we're going to invalidate the entire address space after bootstrapping. No need
        // to flush in between.
        unsafe { flush.ignore() };

        Ok(())
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
    /// Returning `Err` indicates the mapping cannot be established. NOTE: The address space may remain
    /// partially altered. The caller should call *unmap* on the virtual address range upon failure.
    pub fn map_physical_memory(
        &mut self,
        regions: impl Iterator<Item = Range<PhysicalAddress>>,
        active_physmap: &PhysMap,
        chosen_physmap: &PhysMap,
        frame_allocator: impl FrameAllocator,
    ) -> Result<(), AllocError> {
        let attrs = MemoryAttributes::new()
            .with(MemoryAttributes::READ, true)
            .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write);

        for region_phys in regions {
            // NB: use the desired physmap (ie the one used after bootstrapping)
            let region_virt = chosen_physmap.phys_to_virt_range(region_phys.clone());

            let mut flush = Flush::new();

            // Safety: we just created the address space and `BootstrapAllocator` checks its regions to
            // not be overlapping (1.). It will also align regions to at least page size (2., 3.).
            unsafe {
                self.address_space.map_contiguous(
                    region_virt,
                    region_phys.start,
                    attrs,
                    frame_allocator.by_ref(),
                    active_physmap,
                    &mut flush,
                )?;
            }

            // Safety: we're going to invalidate the entire address space after bootstrapping. No need
            // to flush in between.
            unsafe { flush.ignore() };
        }

        Ok(())
    }

    /// Finish the address space bootstrapping phase and activate the address space on this CPU (set
    /// this CPUs page table).
    ///
    /// # Safety
    ///
    /// After this method returns, all pointers become dangling and as such any access through
    /// pre-existing pointers is Undefined Behavior. This includes implicit references by the CPU
    /// such as the instruction pointer.
    ///
    /// This might seem impossible to uphold, except for identity-mappings which we consider valid
    /// even after activating the address space.
    pub unsafe fn finish_bootstrap_and_activate(self) -> HardwareAddressSpace<A> {
        let (arch, root_page_table) = self.address_space.into_parts();

        // Safety: ensured by caller
        unsafe { arch.set_active_table(root_page_table.address()) };

        // NB: this is load-bearing. We need to ensure to flush the entire address space with all
        // CPUs so that it correctly takes effect (especially so if the address space ID was reused).
        arch.fence_all();

        // Safety: we ensured that we have correctly initialized the address space
        unsafe { HardwareAddressSpace::from_parts(arch, root_page_table) }
    }
}
