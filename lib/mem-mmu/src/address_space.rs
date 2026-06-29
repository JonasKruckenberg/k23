// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::convert::Infallible;
use core::range::Range;

use mem_core::arch::{Arch, MapsAt, PageTableEntry, PageTableLevel};
use mem_core::{
    AddressRangeExt, AllocError, FrameAllocator, MemoryAttributes, PageSize, PhysMap,
    PhysicalAddress, VirtualAddress,
};

use crate::flush::Flush;
use crate::table::{Step, Table, marker};

pub struct HardwareAddressSpace<A: Arch> {
    arch: A,
    root_page_table: Table<A, marker::Owned>,
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
        physmap: &PhysMap,
        frame_allocator: impl FrameAllocator,
    ) -> Result<Self, AllocError> {
        let root_page_table = Table::allocate(frame_allocator, physmap, &arch)?;

        Ok(Self {
            arch,
            root_page_table,
        })
    }

    /// Constructs a new `AddressSpace` from its raw components: architecture-specific data and the root table.
    ///
    /// #  Safety
    ///
    /// The caller must ensure the address space defined by `arch`, `root_page_table`, and `physmap`
    /// indeed represents a properly initialized address space according to [`Active`].
    pub unsafe fn from_parts(arch: A, root_page_table: Table<A, marker::Owned>) -> Self {
        Self {
            root_page_table,
            arch,
        }
    }

    /// Decomposes an `AddressSpace` into its raw components: architecture-specific data and the root table.
    pub fn into_parts(self) -> (A, Table<A, marker::Owned>) {
        (self.arch, self.root_page_table)
    }

    /// Decomposes an `AddressSpace` into its raw components: architecture-specific data and the
    /// physical address of the root page table.
    pub fn into_raw_parts(self) -> (A, PhysicalAddress) {
        let (root_page_table, depth) = self.root_page_table.into_raw_parts();
        debug_assert_eq!(depth, 0);
        (self.arch, root_page_table)
    }

    pub fn arch(&self) -> &A {
        &self.arch
    }

    pub const fn granule_size(&self) -> usize {
        A::GRANULE_SIZE
    }

    pub const fn granule_layout(&self) -> Layout {
        A::GRANULE_LAYOUT
    }

    /// Activate the address space on this CPU (set this CPUs page table).
    ///
    /// # Safety
    ///
    /// After this method returns, all pointers become dangling and as such any access through
    /// pre-existing pointers is Undefined Behavior. This includes implicit references by the CPU
    /// such as the instruction pointer.
    ///
    /// This might seem impossible to uphold, except for identity-mappings which we consider valid
    /// even after activating the address space.
    pub unsafe fn activate(&mut self) {
        debug_assert!(
            self.arch.active_table().is_none(),
            "During bootstrapping the machine must have no active page table."
        );

        let Self {
            arch,
            root_page_table,
            ..
        } = self;

        // Safety: ensured by caller
        unsafe { arch.set_active_table(root_page_table.address()) };

        // NB: this is load-bearing. We need to ensure to flush the entire address space with all
        // CPUs so that it correctly takes effect (especially so if the address space ID was reused).
        arch.fence_all();
    }

    /// Return the corresponding [`PhysicalAddress`] and [`MemoryAttributes`] for the given
    /// [`VirtualAddress`] if mapped. The returned [`PageTableLevel`] described the page table level
    /// at which the mapping was found.
    pub fn lookup(
        &self,
        virt: VirtualAddress,
        physmap: &PhysMap,
    ) -> Option<(PhysicalAddress, MemoryAttributes, &'static PageTableLevel)> {
        let mut table = self.root_page_table.borrow();

        for level in A::LEVELS {
            let entry_index = level.pte_index_of(virt);
            // Safety: `pte_index_of` only returns in-bounds indices.
            let entry = unsafe { table.get(entry_index, physmap, &self.arch) };

            if entry.is_table() {
                debug_assert!((table.depth() as usize + 1) < A::LEVELS.len());

                // Safety: We checked the entry is a table above (1.) know the depth is correct (2.).
                table = unsafe { Table::from_raw_parts(entry.address(), table.depth() + 1) };
            } else if entry.is_leaf() {
                let lower_bits = virt.get() & (level.page_size() - 1);

                // `entry.address()` is aligned to `level.page_size()` on all supported architectures
                // meaning the lower bits must already be zero
                return Some((
                    PhysicalAddress::new(entry.address().get() | lower_bits),
                    entry.attributes(),
                    level,
                ));
            } else {
                debug_assert!(entry.is_vacant());
                return None;
            }
        }

        log::warn!(
            "Reached the page table depth limit without finding a vacant or leaf entry. This indicates a malformed page table!"
        );
        // turn this soft warning into a hard panic in debug mode
        debug_assert!(false);
        None
    }

    /// Maps the virtual address range `virt` to *possibly discontiguous* block(s) of physical memory
    /// `phys` with the specified memory attributes.
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
    /// 2. `virt` must be aligned to `S`.
    /// 3. `phys` blocks must be aligned to `S`.
    /// 4. `phys` blocks must in-total be at least as large as `virt`.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the mapping cannot be established. NOTE: The address space may remain
    /// partially altered. The caller should call *unmap* on the virtual address range upon failure.
    pub unsafe fn map<S: PageSize>(
        &mut self,
        mut virt: Range<VirtualAddress>,
        phys: impl ExactSizeIterator<Item = Range<PhysicalAddress>>,
        attributes: MemoryAttributes,
        frame_allocator: impl FrameAllocator,
        physmap: &PhysMap,
        flush: &mut Flush,
    ) -> Result<(), AllocError>
    where
        A: MapsAt<S>,
    {
        for block_phys in phys {
            debug_assert!(!virt.is_empty());

            // Safety: ensured by caller
            unsafe {
                self.map_contiguous::<S>(
                    Range::from_start_len(virt.start, block_phys.len()),
                    block_phys.start,
                    attributes,
                    frame_allocator.by_ref(),
                    physmap,
                    flush,
                )?;
            }

            virt.start = virt.start.add(block_phys.len());
        }

        Ok(())
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
    /// 2. `virt` must be aligned to `S`.
    /// 3. `phys` must be aligned to `S`.
    /// 4. The region pointed to by `phys` must be at least as large as `virt`.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the mapping cannot be established. NOTE: The address space may remain
    /// partially altered. The caller should call *unmap* on the virtual address range upon failure.
    pub unsafe fn map_contiguous<S: PageSize>(
        &mut self,
        virt: Range<VirtualAddress>,
        mut phys: PhysicalAddress,
        attributes: MemoryAttributes,
        frame_allocator: impl FrameAllocator,
        physmap: &PhysMap,
        flush: &mut Flush,
    ) -> Result<(), AllocError>
    where
        A: MapsAt<S>,
    {
        debug_assert!(
            virt.len() >= S::BYTES,
            "address range must span at least one page of size {}",
            S::BYTES,
        );
        debug_assert!(
            virt.start.is_aligned_to(S::BYTES),
            "virtual address {} must be aligned to page size {}",
            virt.start,
            S::BYTES,
        );
        debug_assert!(
            virt.end.is_aligned_to(S::BYTES),
            "virtual address {} must be aligned to page size {}",
            virt.end,
            S::BYTES,
        );
        debug_assert!(
            phys.is_aligned_to(S::BYTES),
            "physical address {phys} must be aligned to page size {}",
            S::BYTES,
        );

        let leaf_depth = <A as MapsAt<S>>::DEPTH;

        let map_contiguous =
            |entry: &mut A::PageTableEntry, step: Step| -> Result<(), AllocError> {
                let Step::Descend { range, depth } = step else {
                    return Ok(());
                };

                // If the entry is a table, just keep walking
                if entry.is_table() {
                    return Ok(());
                }

                debug_assert!(entry.is_vacant());

                if depth == leaf_depth {
                    *entry = <A as Arch>::PageTableEntry::new_leaf(phys, attributes);

                    phys = phys.add(range.len());

                    // TODO fence(modified pages, 0) if attributes includes GLOBAL
                    // TODO we can omit the fence here and lazily change the mapping in the fault handler#
                    flush.invalidate(range);
                } else {
                    let frame = frame_allocator.allocate_contiguous_zeroed(
                        A::GRANULE_LAYOUT,
                        physmap,
                        &self.arch,
                    )?;

                    *entry = <A as Arch>::PageTableEntry::new_table(frame);

                    // TODO fence(all pages, 0) if attributes includes GLOBAL
                    flush.invalidate_all();
                }

                Ok(())
            };

        self.root_page_table
            .borrow_mut()
            .visit_mut(virt, physmap, &self.arch, map_contiguous)?;

        Ok(())
    }

    /// Remaps the virtual address range `virt` to new *possibly discontiguous* block(s) of physical
    /// memory `phys`. The old physical memory region is not freed.
    ///
    /// Note that this method **does not** establish any ordering between address space modification
    /// and accesses through the mapping, nor does it imply a page table cache flush. To ensure the
    /// updated mapping is visible to the calling CPU you must call [`flush`][Flush::flush] on the returned [`Flush`].
    ///
    /// After the modifications have been synchronized with current execution, all accesses to the virtual
    /// address range will translate to accesses of the new physical address range.
    ///
    /// # Safety
    ///
    /// 1. The entire range `virt` must be mapped with `S`-sized leaves.
    /// 2. `virt` must be aligned to `S`.
    /// 3. `phys` blocks must be aligned to `S`.
    /// 4. `phys` blocks must in-total be at least as large as `virt`.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the mapping cannot be established. NOTE: The address space may remain
    /// partially altered. The caller should call *unmap* on the virtual address range upon failure.
    pub unsafe fn remap<S: PageSize>(
        &mut self,
        mut virt: Range<VirtualAddress>,
        phys: impl ExactSizeIterator<Item = Range<PhysicalAddress>>,
        physmap: &PhysMap,
        flush: &mut Flush,
    ) -> Result<(), AllocError>
    where
        A: MapsAt<S>,
    {
        for block_phys in phys {
            debug_assert!(!virt.is_empty());

            // Safety: ensured by caller
            unsafe {
                self.remap_contiguous::<S>(
                    Range::from_start_len(virt.start, block_phys.len()),
                    block_phys.start,
                    physmap,
                    flush,
                );
            }

            virt.start = virt.start.add(block_phys.len());
        }

        Ok(())
    }

    /// Remaps the virtual address range `virt` to a new continuous region of physical memory starting
    /// at `phys`. The old physical memory region is not freed.
    ///
    /// Note that this method **does not** establish any ordering between address space modification
    /// and accesses through the mapping, nor does it imply a page table cache flush. To ensure the
    /// updated mapping is visible to the calling CPU you must call [`flush`][Flush::flush] on the returned [`Flush`].
    ///
    /// After the modifications have been synchronized with current execution, all accesses to the virtual
    /// address range will translate to accesses of the new physical address range.
    ///
    /// # Safety
    ///
    /// 1. The entire range `virt` must be mapped with `S`-sized leaves.
    /// 2. `virt` must be aligned to `S`.
    /// 3. `phys` must be aligned to `S`.
    /// 4. The region pointed to by `phys` must be at least as large as `virt`.
    pub unsafe fn remap_contiguous<S: PageSize>(
        &mut self,
        virt: Range<VirtualAddress>,
        mut phys: PhysicalAddress,
        physmap: &PhysMap,
        flush: &mut Flush,
    ) where
        A: MapsAt<S>,
    {
        debug_assert!(
            virt.len() >= S::BYTES,
            "address range must span at least one page of size {}",
            S::BYTES,
        );
        debug_assert!(
            virt.start.is_aligned_to(S::BYTES),
            "virtual address {} must be aligned to page size {}",
            virt.start,
            S::BYTES,
        );
        debug_assert!(
            phys.is_aligned_to(S::BYTES),
            "physical address {phys} must be aligned to page size {}",
            S::BYTES,
        );

        let leaf_depth = <A as MapsAt<S>>::DEPTH;

        let remap_contiguous =
            |entry: &mut A::PageTableEntry, step: Step| -> Result<(), Infallible> {
                let Step::Descend { range, depth } = step else {
                    return Ok(());
                };

                debug_assert!(!entry.is_vacant());

                if entry.is_leaf() {
                    debug_assert!(
                        depth == leaf_depth,
                        "virtual address range must be mapped at page size {}",
                        S::BYTES,
                    );

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
                .visit_mut(virt, physmap, &self.arch, remap_contiguous)
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
    /// 1. The entire range `virt` must be mapped with `S`-sized leaves.
    /// 2. `virt` must be aligned to `S`.
    pub unsafe fn set_attributes<S: PageSize>(
        &mut self,
        virt: Range<VirtualAddress>,
        attributes: MemoryAttributes,
        physmap: &PhysMap,
        flush: &mut Flush,
    ) where
        A: MapsAt<S>,
    {
        debug_assert!(
            virt.len() >= S::BYTES,
            "address range must span at least one page of size {}",
            S::BYTES,
        );
        debug_assert!(
            virt.start.is_aligned_to(S::BYTES),
            "virtual address {} must be aligned to page size {}",
            virt.start,
            S::BYTES,
        );

        let leaf_depth = <A as MapsAt<S>>::DEPTH;

        let set_attributes =
            |entry: &mut A::PageTableEntry, step: Step| -> Result<(), Infallible> {
                let Step::Descend { range, depth } = step else {
                    return Ok(());
                };

                debug_assert!(!entry.is_vacant());

                if entry.is_leaf() {
                    debug_assert!(
                        depth == leaf_depth,
                        "virtual address range must be mapped at page size {}",
                        S::BYTES,
                    );

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
                .visit_mut(virt, physmap, &self.arch, set_attributes)
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
    /// 1. The entire range `virt` must be mapped with `S`-sized leaves.
    /// 2. `virt` must be aligned to `S`.
    pub unsafe fn unmap<S: PageSize>(
        &mut self,
        virt: Range<VirtualAddress>,
        frame_allocator: impl FrameAllocator,
        physmap: &PhysMap,
        flush: &mut Flush,
    ) where
        A: MapsAt<S>,
    {
        debug_assert!(
            virt.len() >= S::BYTES,
            "address range must span at least one page of size {}",
            S::BYTES,
        );
        debug_assert!(
            virt.start.is_aligned_to(S::BYTES),
            "virtual address {} must be aligned to page size {}",
            virt.start,
            S::BYTES,
        );

        let leaf_depth = <A as MapsAt<S>>::DEPTH;

        let unmap = |entry: &mut A::PageTableEntry, step: Step| -> Result<(), Infallible> {
            match step {
                // Descending: vacate the leaf entries covering `virt`.
                Step::Descend { range, depth } => {
                    debug_assert!(!entry.is_vacant());

                    if entry.is_leaf() {
                        debug_assert!(
                            depth == leaf_depth,
                            "virtual address range must be mapped at page size {}",
                            S::BYTES,
                        );

                        *entry = A::PageTableEntry::VACANT;

                        // TODO fence(modified pages, 0) if attributes includes GLOBAL
                        flush.invalidate(range);
                    }
                }
                // Ascending: vacating its entries may have emptied the subtable; if so,
                // free it and clear the entry that pointed at it.
                Step::Ascend { child_depth } => {
                    // Safety: `entry` is the parent of the just-visited child table,
                    // which sits at `child_depth`.
                    let child: Table<A, marker::Mut<'_>> =
                        unsafe { Table::from_raw_parts(entry.address(), child_depth) };

                    if child.is_empty(physmap, &self.arch) {
                        let frame = entry.address();

                        *entry = A::PageTableEntry::VACANT;

                        // Safety: tables are always allocated through the frame allocator, and
                        // are always exactly one frame in size.
                        unsafe {
                            frame_allocator.deallocate(frame, A::GRANULE_LAYOUT);
                        }

                        // TODO fence(all pages, 0) if attributes includes GLOBAL
                        flush.invalidate_all();
                    }
                }
            }

            Ok(())
        };

        // Safety: `unmap` is infallible
        unsafe {
            self.root_page_table
                .borrow_mut()
                .visit_mut(virt, physmap, &self.arch, unmap)
                .unwrap_unchecked();
        }
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
    /// 2. `phys` must be aligned to `S`.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the mapping cannot be established and the address space remains
    /// unaltered.
    pub unsafe fn map_identity<S: PageSize>(
        &mut self,
        phys: Range<PhysicalAddress>,
        attributes: MemoryAttributes,
        frame_allocator: impl FrameAllocator,
        physmap: &PhysMap,
        flush: &mut Flush,
    ) -> Result<(), AllocError>
    where
        A: MapsAt<S>,
    {
        let virt = Range {
            start: VirtualAddress::new(phys.start.get()),
            end: VirtualAddress::new(phys.end.get()),
        };

        // Safety: ensured by caller.
        unsafe {
            self.map_contiguous::<S>(
                virt,
                phys.start,
                attributes,
                frame_allocator,
                physmap,
                flush,
            )?;
        }

        Ok(())
    }
}
