mod frame_allocator;

use core::mem;
use core::ops::Range;

pub use frame_allocator::{BootstrapAllocator, FreeRegions, UsedRegions, DEFAULT_MAX_REGIONS};

use crate::arch::Arch;
use crate::flush::Flush;
use crate::{
    AllocError, FrameAllocator, HardwareAddressSpace, MemoryAttributes, PhysicalAddress,
    PhysicalMemoryMapping, VirtualAddress, WriteOrExecute,
};

pub struct Bootstrap<S> {
    pub(crate) address_space: S,
    pub(crate) future_physmap: PhysicalMemoryMapping,
}

impl<A: Arch> Bootstrap<HardwareAddressSpace<A>> {
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
        let attrs = MemoryAttributes::new()
            .with(MemoryAttributes::READ, true)
            .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write);

        for region_phys in frame_allocator.regions() {
            // NB: use the "future" physical memory mapping (ie after bootstrapping)
            let region_virt = self.future_physmap.phys_to_virt_range(region_phys.clone());

            // Safety: we just created the address space and `BootstrapAllocator` checks its regions to
            // not be overlapping (1.). It will also align regions to at least page size (2., 3.).
            unsafe {
                self.address_space.map_contiguous(
                    region_virt,
                    region_phys.start,
                    attrs,
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
    pub unsafe fn map_identity<F>(
        &mut self,
        phys: Range<PhysicalAddress>,
        attributes: MemoryAttributes,
        frame_allocator: F,
        flush: &mut Flush,
    ) -> Result<(), AllocError>
    where
        F: FrameAllocator,
    {
        let virt = unsafe {
            Range {
                start: mem::transmute::<PhysicalAddress, VirtualAddress>(phys.start),
                end: mem::transmute::<PhysicalAddress, VirtualAddress>(phys.end),
            }
        };

        unsafe {
            self.address_space
                .map_contiguous(virt, phys.start, attributes, frame_allocator, flush)
        }
    }

    /// Finish the address space bootstrapping phase and activate the address space on this CPU (set
    /// this CPUs page table).
    ///
    /// # Safety
    ///
    /// After this method returns, all pointers become dangling and as such any access through
    /// pre-existing pointers is Undefined Behaviour. This includes implicit references by the CPU
    /// such as the instruction pointer.
    pub unsafe fn finish_bootstrap_and_activate(self) -> HardwareAddressSpace<A> {
        let (arch, root_table, _) = self.address_space.into_parts();

        unsafe { arch.set_active_table(root_table.address()) };

        HardwareAddressSpace::from_parts(arch, root_table, self.future_physmap)
    }
}
