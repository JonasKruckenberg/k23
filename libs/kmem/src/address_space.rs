use core::convert::Infallible;
use core::ops::Range;

use crate::arch::{Arch, PageTableEntry, PageTableLevel};
use crate::bootstrap::{Bootstrap, BootstrapAllocator};
use crate::flush::Flush;
use crate::physmap::PhysMap;
use crate::table::{Table, marker};
use crate::utils::{PageTableEntries, page_table_entries_for};
use crate::{
    AddressRangeExt, AllocError, FrameAllocator, MemoryAttributes, PhysicalAddress, VirtualAddress,
};

pub struct HardwareAddressSpace<A: Arch> {
    arch: A,
    root_page_table: Table<A, marker::Owned>,
    physmap: PhysMap,
}

impl<A: Arch> HardwareAddressSpace<A> {
    /// Constructs a new `AddressSpace` with a freshly allocated root page table
    /// that may be used during address space bringup in the `loader`.
    ///
    /// # Errors
    ///
    /// Returns `Err(AllocError)` when allocating the root page table fails.
    pub fn new(
        arch: A,
        physmap: PhysMap,
        frame_allocator: impl FrameAllocator,
        flush: &mut Flush,
    ) -> Result<Self, AllocError> {
        let root_page_table = Table::allocate(frame_allocator, &physmap, &arch)?;

        flush.invalidate_all();

        Ok(Self {
            physmap,
            root_page_table,
            arch,
        })
    }

    /// Constructs a new *bootstrapping* `AddressSpace` with a freshly allocated root page table
    /// that may be used during address space bringup in the `loader`.
    ///
    /// # Errors
    ///
    /// Returns `Err(AllocError)` when allocating the root page table fails.
    pub fn new_bootstrap<R: lock_api::RawMutex>(
        arch: A,
        future_physmap: PhysMap,
        frame_allocator: &BootstrapAllocator<R>,
        flush: &mut Flush,
    ) -> Result<Bootstrap<Self>, AllocError> {
        let address_space = Self::new(arch, PhysMap::new_bootstrap(), frame_allocator, flush)?;

        Ok(Bootstrap {
            address_space,
            future_physmap,
        })
    }

    /// Constructs a new `AddressSpace` from its raw components: architecture-specific data and the root table.
    pub fn from_parts(arch: A, root_page_table: Table<A, marker::Owned>, physmap: PhysMap) -> Self {
        Self {
            physmap,
            root_page_table,
            arch,
        }
    }

    /// Decomposes an `AddressSpace` into its raw components: architecture-specific data and the root table.
    pub fn into_parts(self) -> (A, Table<A, marker::Owned>, PhysMap) {
        (self.arch, self.root_page_table, self.physmap)
    }

    pub fn arch(&self) -> &A {
        &self.arch
    }

    /// Activate the address space on this CPU (set this CPUs page table).
    ///
    /// # Safety
    ///
    /// After this method returns, all pointers become dangling and as such any access through
    /// pre-existing pointers is Undefined Behaviour. This includes implicit references by the CPU
    /// such as the instruction pointer.
    pub unsafe fn activate(&self) {
        todo!()
        // unsafe { (self.vtable.activate)(self.raw, self.root_page_table) }
    }

    /// Return the corresponding [`PhysicalAddress`] and [`MemoryAttributes`] for the given
    /// [`VirtualAddress`] if mapped. The returned [`PageTableLevel`] described the page table level
    /// at which the mapping was found.
    pub fn lookup(
        &self,
        virt: VirtualAddress,
    ) -> Option<(PhysicalAddress, MemoryAttributes, &'static PageTableLevel)> {
        let mut table = self.root_page_table.borrow();

        for level in A::LEVELS {
            let entry_index = level.pte_index_of(virt);
            // Safety: `pte_index_of` only returns in-bounds indices.
            let entry = unsafe { table.get(entry_index, &self.physmap, &self.arch) };

            if entry.is_table() {
                // Safety: We checked the entry is a table above (1.) know the depth is correct (2.).
                table = unsafe { Table::from_raw_parts(entry.address(), table.depth() + 1) };
            } else if entry.is_leaf() {
                return Some((entry.address(), entry.attributes(), level));
            } else {
                debug_assert!(entry.is_vacant());
                return None;
            }
        }

        unreachable!()
    }

    /// Maps the virtual address range `virt` to a continuous region of physical memory starting at `phys`
    /// with the specified memory attributes.
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
    /// 1. The entire range `virt` must be unmapped.
    /// 2. `virt` must be aligned to at least the smallest architecture block size.
    /// 3. `phys` must be aligned to at least the smallest architecture block size.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the mapping cannot be established and the address space remains
    /// unaltered.
    pub unsafe fn map_contiguous(
        &mut self,
        virt: Range<VirtualAddress>,
        mut phys: PhysicalAddress,
        attributes: MemoryAttributes,
        frame_allocator: impl FrameAllocator,
        flush: &mut Flush,
    ) -> Result<(), AllocError> {
        debug_assert!(
            virt.len() >= A::GRANULE_SIZE,
            "address range span be at least one page"
        );
        debug_assert!(
            virt.start.is_aligned_to(A::GRANULE_SIZE,),
            "virtual address {} must be aligned to at least page size {}",
            virt.start,
            A::GRANULE_SIZE,
        );
        debug_assert!(
            phys.is_aligned_to(A::GRANULE_SIZE),
            "physical address {phys} must be aligned to at least page size {}",
            A::GRANULE_SIZE,
        );

        let map_contiguous = |entry: &mut A::PageTableEntry,
                              range: Range<VirtualAddress>,
                              level: &'static PageTableLevel|
         -> Result<(), AllocError> {
            debug_assert!(entry.is_vacant());
            debug_assert!(!entry.is_leaf() && !entry.is_table());

            if level.can_map(range.start, phys, range.len()) {
                *entry = <A as Arch>::PageTableEntry::new_leaf(phys, attributes);

                phys = phys.add(range.len());

                // TODO fence(modified pages, 0) if attributes includes GLOBAL
                // TODO we can omit the fence here and lazily change the mapping in the fault handler#
                flush.invalidate(range);
            } else {
                let frame = frame_allocator.allocate_contiguous_zeroed(
                    A::GRANULE_LAYOUT,
                    &self.physmap,
                    &self.arch,
                )?;

                *entry = <A as Arch>::PageTableEntry::new_table(frame);

                // TODO fence(all pages, 0) if attributes includes GLOBAL
                flush.invalidate_all();
            }

            Ok(())
        };

        self.root_page_table.borrow_mut().visit_mut(
            virt,
            &self.physmap,
            &self.arch,
            map_contiguous,
        )?;

        Ok(())
    }

    /// Remaps the virtual address range `virt` to a new continuous region of physical memory start
    /// at `phys`. The old physical memory region is not freed.
    ///
    /// Note that this method **does not** establish any ordering between address space modification
    /// and accesses through the mapping, nor does it imply a page table cache flush. To ensure the
    /// updated mapping is visible to the calling CPU you must call [`flush`][Flush::flush] on the returned `[Flush`].
    ///
    /// After the modifications have been synchronized with current execution, all accesses to the virtual
    /// address range will translate to accesses of the new physical address range.
    ///
    /// # Safety
    ///
    /// 1. The entire range `virt` must be mapped.
    /// 2. `virt` must be aligned to at least the smallest architecture block size.
    /// 3. `phys` must be aligned to `at least the smallest architecture block size.
    pub unsafe fn remap_contiguous(
        &mut self,
        virt: Range<VirtualAddress>,
        mut phys: PhysicalAddress,
        flush: &mut Flush,
    ) {
        debug_assert!(
            virt.len() >= A::GRANULE_SIZE,
            "address range span be at least one page"
        );
        debug_assert!(
            virt.start.is_aligned_to(A::GRANULE_SIZE),
            "virtual address {} must be aligned to at least page size {}",
            virt.start,
            A::GRANULE_SIZE,
        );
        debug_assert!(
            phys.is_aligned_to(A::GRANULE_SIZE),
            "physical address {phys} must be aligned to at least page size {}",
            A::GRANULE_SIZE,
        );

        let remap_contiguous = |entry: &mut A::PageTableEntry,
                                range: Range<VirtualAddress>,
                                _level: &'static PageTableLevel|
         -> Result<(), Infallible> {
            debug_assert!(!entry.is_vacant());

            if entry.is_leaf() {
                *entry = A::PageTableEntry::new_leaf(phys, entry.attributes());

                phys = phys.add(range.len());

                // TODO fence(modified pages, 0) if attributes includes GLOBAL
                flush.invalidate(range);
            }

            Ok(())
        };

        // Safety: `remap_contiguous` is infallible
        unsafe {
            self.root_page_table
                .borrow_mut()
                .visit_mut(virt, &self.physmap, &self.arch, remap_contiguous)
                .unwrap_unchecked();
        }
    }

    /// Set the [`MemoryAttributes`] for the virtual address range `virt`.
    ///
    /// Note that this method **does not** establish any ordering between address space modification
    /// and accesses through the mapping, nor does it imply a page table cache flush. To ensure the
    /// updated mapping is visible to the calling CPU you must call [`flush`][Flush::flush] on the returned `[Flush`].
    ///
    /// After the modifications have been synchronized with current execution, all accesses to the virtual
    /// address range adhere to the access rules established by the `MemoryAttributes`.
    ///
    /// # Safety
    ///
    /// 1. The entire range `virt` must be mapped.
    /// 2. `virt` must be aligned to at least the smallest architecture block size.
    /// 3. `phys` must be aligned to `at least the smallest architecture block size.
    pub unsafe fn set_attributes(
        &mut self,
        virt: Range<VirtualAddress>,
        attributes: MemoryAttributes,
        flush: &mut Flush,
    ) {
        debug_assert!(
            virt.len() >= A::GRANULE_SIZE,
            "address range span be at least one page"
        );
        debug_assert!(
            virt.start.is_aligned_to(A::GRANULE_SIZE),
            "virtual address {} must be aligned to at least page size {}",
            virt.start,
            A::GRANULE_SIZE,
        );

        let set_attributes = |entry: &mut A::PageTableEntry,
                              range: Range<VirtualAddress>,
                              _level: &'static PageTableLevel|
         -> Result<(), Infallible> {
            debug_assert!(!entry.is_vacant());

            if entry.is_leaf() {
                *entry = A::PageTableEntry::new_leaf(entry.address(), attributes);

                // TODO fence(modified pages, 0) if attributes includes GLOBAL
                // TODO we can omit the fence here IF the attributes are MORE PERMISSIVE than before and
                //  lazily change the mapping in the fault handler
                flush.invalidate(range);
            }

            Ok(())
        };

        // Safety: `set_attributes` is infallible
        unsafe {
            self.root_page_table
                .borrow_mut()
                .visit_mut(virt, &self.physmap, &self.arch, set_attributes)
                .unwrap_unchecked();
        }
    }

    /// Unmaps the virtual address range `virt`.
    ///
    /// Note that this method **does not** establish any ordering between address space modification
    /// and accesses through the mapping, nor does it imply a page table cache flush. To ensure the
    /// removal is visible to the calling CPU you must call [`flush`][Flush::flush] on the returned `[Flush`].
    ///
    /// After the modifications have been synchronized with current execution, all accesses to the virtual
    /// address range will cause a page fault.
    ///
    /// # Safety
    ///
    /// 1. The entire range `virt` must be mapped.
    /// 2. `virt` must be aligned to at least the smallest architecture block size.
    /// 3. `phys` must be aligned to `at least the smallest architecture block size.
    pub unsafe fn unmap(
        &mut self,
        virt: Range<VirtualAddress>,
        frame_allocator: impl FrameAllocator,
        flush: &mut Flush,
    ) {
        debug_assert!(
            virt.len() >= A::GRANULE_SIZE,
            "address range span be at least one page"
        );
        debug_assert!(
            virt.start.is_aligned_to(A::GRANULE_SIZE),
            "virtual address {} must be aligned to at least page size {}",
            virt.start,
            A::GRANULE_SIZE,
        );

        let table = self.root_page_table.borrow_mut();

        Self::unmap_inner(
            table,
            virt,
            &self.physmap,
            &self.arch,
            frame_allocator,
            flush,
        );
    }

    fn unmap_inner(
        mut table: Table<A, marker::Mut<'_>>,
        range: Range<VirtualAddress>,
        physmap: &PhysMap,
        arch: &A,
        frame_allocator: impl FrameAllocator,
        flush: &mut Flush,
    ) {
        let entries: PageTableEntries<A> = page_table_entries_for(range.clone(), table.level());

        for (entry_index, range) in entries {
            // Safety: `page_table_entries_for` only returns in-bounds indices.
            let mut entry = unsafe { table.get(entry_index, physmap, arch) };
            debug_assert!(!entry.is_vacant());

            if entry.is_leaf() {
                entry = A::PageTableEntry::VACANT;

                // TODO fence(modified pages, 0) if attributes includes GLOBAL
                flush.invalidate(range);
            } else {
                // Safety: We checked the entry is a table above (1.) know the depth is correct (2.).
                let mut subtable: Table<A, marker::Mut<'_>> =
                    unsafe { Table::from_raw_parts(entry.address(), table.depth() + 1) };

                Self::unmap_inner(
                    subtable.reborrow_mut(),
                    range,
                    physmap,
                    arch,
                    frame_allocator.by_ref(),
                    flush,
                );

                if subtable.is_empty(physmap, arch) {
                    let frame = entry.address();

                    entry = A::PageTableEntry::VACANT;

                    // Safety: tables are always allocated through the frame allocator, and are always
                    // exactly one frame in size.
                    unsafe {
                        frame_allocator.deallocate(frame, A::GRANULE_LAYOUT);
                    }

                    // TODO fence(all pages, 0) if attributes includes GLOBAL
                    flush.invalidate_all();
                }
            }

            // Safety: `page_table_entries_for` only returns in-bounds indices.
            unsafe {
                table.set(entry_index, entry, physmap, arch);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use crate::address_range::AddressRangeExt;
    use crate::arch::Arch;
    use crate::flush::Flush;
    use crate::frame_allocator::FrameAllocator;
    use crate::test_utils::{BootstrapResult, MachineBuilder};
    use crate::{MemoryAttributes, VirtualAddress, WriteOrExecute, archtest};

    archtest! {
        #[test]
        fn map<A: Arch>() {
            let BootstrapResult {
                mut address_space,
                frame_allocator,
                ..
            } = MachineBuilder::<A, parking_lot::RawMutex, _>::new()
                .with_memory_regions([0xA000])
                .finish_and_bootstrap()
                .unwrap();

            let frame = frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();

            let page = Range::from_start_len(VirtualAddress::new(0x7000), A::GRANULE_SIZE);

            let mut flush = Flush::new();
            unsafe {
                address_space
                    .map_contiguous(
                        page.clone(),
                        frame,
                        MemoryAttributes::new().with(MemoryAttributes::READ, true),
                        frame_allocator.by_ref(),
                        &mut flush,
                    )
                    .unwrap();
            }
            flush.flush(address_space.arch());

            let (phys, attrs, lvl) = address_space.lookup(page.start).unwrap();

            assert_eq!(phys, frame);
            assert_eq!(attrs.allows_read(), true);
            assert_eq!(attrs.allows_write(), false);
            assert_eq!(attrs.allows_execution(), false);
            assert_eq!(lvl.page_size(), 4096);
        }

        #[test]
        fn remap<A: Arch>() {
            let BootstrapResult {
                mut address_space,
                frame_allocator,
                ..
            } = MachineBuilder::<A, parking_lot::RawMutex, _>::new()
                .with_memory_regions([0xB000])
                .finish_and_bootstrap()
                .unwrap();

            let frame = frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();

            let page = Range::from_start_len(
                VirtualAddress::new(0x7000),
                A::GRANULE_SIZE,
            );

            let mut flush = Flush::new();
            unsafe {
                address_space
                    .map_contiguous(
                        page.clone(),
                        frame,
                        MemoryAttributes::new().with(MemoryAttributes::READ, true),
                        frame_allocator.by_ref(),
                        &mut flush,
                    )
                    .unwrap();
            }
            flush.flush(address_space.arch());

            let (phys, attrs, lvl) = address_space.lookup(page.start).unwrap();

            assert_eq!(phys, frame);
            assert_eq!(attrs.allows_read(), true);
            assert_eq!(attrs.allows_write(), false);
            assert_eq!(attrs.allows_execution(), false);
            assert_eq!(lvl.page_size(), 4096);

            // ===== the actual remap part =====

            let new_frame = frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();

            let mut flush = Flush::new();
            unsafe {
                address_space.remap_contiguous(page.clone(), new_frame, &mut flush);
            }
            flush.flush(address_space.arch());

            let (phys, attrs, lvl) = address_space.lookup(page.start).unwrap();

            assert_eq!(phys, new_frame);
            assert_eq!(attrs.allows_read(), true);
            assert_eq!(attrs.allows_write(), false);
            assert_eq!(attrs.allows_execution(), false);
            assert_eq!(lvl.page_size(), 4096);
        }

        #[test]
        fn set_attributes<A: Arch>() {
            let BootstrapResult {
                mut address_space,
                frame_allocator,
                ..
            } = MachineBuilder::<A, parking_lot::RawMutex, _>::new()
                .with_memory_regions([0xB000])
                .finish_and_bootstrap()
                .unwrap();

            let frame = frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();

            let page = Range::from_start_len(
                VirtualAddress::new(0x7000),
                A::GRANULE_SIZE
            );

            let mut flush = Flush::new();
            unsafe {
                address_space
                    .map_contiguous(
                        page.clone(),
                        frame,
                        MemoryAttributes::new().with(MemoryAttributes::READ, true),
                        frame_allocator.by_ref(),
                        &mut flush,
                    )
                    .unwrap();
            }
            flush.flush(address_space.arch());

            let (phys, attrs, lvl) = address_space.lookup(page.start).unwrap();

            assert_eq!(phys, frame);
            assert_eq!(attrs.allows_read(), true);
            assert_eq!(attrs.allows_write(), false);
            assert_eq!(attrs.allows_execution(), false);
            assert_eq!(lvl.page_size(), 4096);

            // ===== the actual remap part =====

            let mut flush = Flush::new();
            unsafe {
                address_space.set_attributes(
                    page.clone(),
                    MemoryAttributes::new()
                        .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Execute),
                    &mut flush,
                );
            }
            flush.flush(address_space.arch());

            let (phys, attrs, lvl) = address_space.lookup(page.start).unwrap();

            assert_eq!(phys, frame);
            assert_eq!(attrs.allows_read(), false);
            assert_eq!(attrs.allows_write(), false);
            assert_eq!(attrs.allows_execution(), true);
            assert_eq!(lvl.page_size(), 4096);
        }
    }
}
