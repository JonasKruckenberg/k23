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
use crate::table::{Table, Visitor, marker};

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
        phys: PhysicalAddress,
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

        let mut visitor = MapVisitor {
            phys,
            attributes,
            frame_allocator,
            flush,
        };

        self.root_page_table
            .borrow_mut()
            .visit::<S, _>(virt, physmap, &self.arch, &mut visitor)?;

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
        phys: PhysicalAddress,
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

        let mut visitor = RemapVisitor { phys, flush };

        // Safety: `RemapVisitor` is infallible.
        unsafe {
            self.root_page_table
                .borrow_mut()
                .visit::<S, _>(virt, physmap, &self.arch, &mut visitor)
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

        let mut visitor = SetAttributesVisitor { attributes, flush };

        // Safety: `SetAttributesVisitor` is infallible.
        unsafe {
            self.root_page_table
                .borrow_mut()
                .visit::<S, _>(virt, physmap, &self.arch, &mut visitor)
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

        let mut visitor = UnmapVisitor {
            frame_allocator,
            flush,
        };

        // Safety: `UnmapVisitor` is infallible.
        unsafe {
            self.root_page_table
                .borrow_mut()
                .visit::<S, _>(virt, physmap, &self.arch, &mut visitor)
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

/// [`Visitor`] for [`map_contiguous`](HardwareAddressSpace::map_contiguous)
struct MapVisitor<'a, F> {
    phys: PhysicalAddress,
    attributes: MemoryAttributes,
    frame_allocator: F,
    flush: &'a mut Flush,
}

impl<A, S, F> Visitor<A, S> for MapVisitor<'_, F>
where
    A: MapsAt<S>,
    S: PageSize,
    F: FrameAllocator,
{
    type Error = AllocError;

    fn descend(
        &mut self,
        table: &mut Table<A, marker::Mut<'_>>,
        index: u16,
        physmap: &PhysMap,
        arch: &A,
    ) -> Result<Option<PhysicalAddress>, AllocError> {
        // Safety: the walk only descends through in-bounds indices.
        let entry = unsafe { table.get(index, physmap, arch) };

        // If a table already exists: simply descend into it
        if entry.is_table() {
            return Ok(Some(entry.address()));
        }

        debug_assert!(entry.is_vacant());

        // If no table exists: allocate the intermediate table to descend into.
        let frame =
            self.frame_allocator
                .allocate_contiguous_zeroed(A::GRANULE_LAYOUT, physmap, arch)?;

        // Safety: the walk only descends through in-bounds indices.
        unsafe { table.set(index, A::PageTableEntry::new_table(frame), physmap, arch) };

        // TODO fence(all pages, 0) if attributes includes GLOBAL
        self.flush.invalidate_all();

        Ok(Some(frame))
    }

    fn fill(
        &mut self,
        table: &mut Table<A, marker::Mut<'_>>,
        first: u16,
        count: u16,
        va: VirtualAddress,
        physmap: &PhysMap,
        arch: &A,
    ) -> Result<(), AllocError> {
        debug_assert!((first as usize + count as usize) <= table.level().entries() as usize);

        let mut entry_virt = table.entry_address(first, physmap);
        let mut phys = self.phys;

        for _ in 0..count {
            // Precondition: the range is unmapped, so every slot is vacant.
            // Safety: `entry_virt` is within the covered run, in-bounds and aligned.
            debug_assert!(unsafe { arch.read::<A::PageTableEntry>(entry_virt) }.is_vacant());

            let leaf = A::PageTableEntry::new_leaf(phys, self.attributes);
            // Safety: `entry_virt` is within the covered run, in-bounds and aligned.
            unsafe { arch.write(entry_virt, leaf) };

            entry_virt = entry_virt.add(size_of::<A::PageTableEntry>());
            phys = phys.add(S::BYTES);
        }

        self.phys = phys;

        // TODO fence(modified pages, 0) if attributes includes GLOBAL
        // TODO we can omit the fence here and lazily change the mapping in the fault handler
        self.flush
            .invalidate(Range::from_start_len(va, count as usize * S::BYTES));

        Ok(())
    }
}

/// [`Visitor`] for [`remap_contiguous`](HardwareAddressSpace::remap_contiguous)
struct RemapVisitor<'a> {
    phys: PhysicalAddress,
    flush: &'a mut Flush,
}

impl<A, S> Visitor<A, S> for RemapVisitor<'_>
where
    A: MapsAt<S>,
    S: PageSize,
{
    type Error = Infallible;

    fn fill(
        &mut self,
        table: &mut Table<A, marker::Mut<'_>>,
        first: u16,
        count: u16,
        va: VirtualAddress,
        physmap: &PhysMap,
        arch: &A,
    ) -> Result<(), Infallible> {
        let mut entry_virt = table.entry_address(first, physmap);
        let mut phys = self.phys;

        for _ in 0..count {
            // Safety: `entry_virt` is within the covered run, in-bounds and aligned.
            let old = unsafe { arch.read::<A::PageTableEntry>(entry_virt) };
            debug_assert!(
                old.is_leaf(),
                "virtual address range must be mapped at page size {}",
                S::BYTES,
            );

            let new = A::PageTableEntry::new_leaf(phys, old.attributes());
            // Safety: `entry_virt` is within the covered run, in-bounds and aligned.
            unsafe { arch.write(entry_virt, new) };

            entry_virt = entry_virt.add(size_of::<A::PageTableEntry>());
            phys = phys.add(S::BYTES);
        }

        self.phys = phys;

        // TODO fence(modified pages, 0) if attributes includes GLOBAL
        self.flush
            .invalidate(Range::from_start_len(va, count as usize * S::BYTES));

        Ok(())
    }
}

/// [`Visitor`] for [`set_attributes`](HardwareAddressSpace::set_attributes)
struct SetAttributesVisitor<'a> {
    attributes: MemoryAttributes,
    flush: &'a mut Flush,
}

impl<A, S> Visitor<A, S> for SetAttributesVisitor<'_>
where
    A: MapsAt<S>,
    S: PageSize,
{
    type Error = Infallible;

    fn fill(
        &mut self,
        table: &mut Table<A, marker::Mut<'_>>,
        first: u16,
        count: u16,
        va: VirtualAddress,
        physmap: &PhysMap,
        arch: &A,
    ) -> Result<(), Infallible> {
        let mut entry_virt = table.entry_address(first, physmap);

        for _ in 0..count {
            // Safety: `entry_virt` is within the covered run, in-bounds and aligned.
            let old = unsafe { arch.read::<A::PageTableEntry>(entry_virt) };
            debug_assert!(
                old.is_leaf(),
                "virtual address range must be mapped at page size {}",
                S::BYTES,
            );

            let new = A::PageTableEntry::new_leaf(old.address(), self.attributes);
            // Safety: `entry_virt` is within the covered run, in-bounds and aligned.
            unsafe { arch.write(entry_virt, new) };

            entry_virt = entry_virt.add(size_of::<A::PageTableEntry>());
        }

        // TODO fence(modified pages, 0) if attributes includes GLOBAL
        // TODO we can omit the fence here IF the attributes are MORE PERMISSIVE than before and
        //  lazily change the mapping in the fault handler
        self.flush
            .invalidate(Range::from_start_len(va, count as usize * S::BYTES));

        Ok(())
    }
}

/// [`Visitor`] for [`unmap`](HardwareAddressSpace::unmap)
struct UnmapVisitor<'a, F> {
    frame_allocator: F,
    flush: &'a mut Flush,
}

impl<A, S, F> Visitor<A, S> for UnmapVisitor<'_, F>
where
    A: MapsAt<S>,
    S: PageSize,
    F: FrameAllocator,
{
    type Error = Infallible;

    fn ascend(
        &mut self,
        table: &mut Table<A, marker::Mut<'_>>,
        index: u16,
        child_base: PhysicalAddress,
        child_depth: u8,
        physmap: &PhysMap,
        arch: &A,
    ) -> Result<(), Infallible> {
        // `ascend` is called after we may have run `fill` to free all its pages on the subtable.
        // If its empty now, clear and free the table itself.

        // Safety: `child_base`/`child_depth` name the just-visited child table, and we
        // inherit `table`'s mutable access to the tree.
        let child: Table<A, marker::Mut<'_>> =
            unsafe { Table::from_raw_parts(child_base, child_depth) };

        if child.is_empty(physmap, arch) {
            // Safety: the walk only ascends through in-bounds indices.
            unsafe { table.set(index, A::PageTableEntry::VACANT, physmap, arch) };

            // Safety: tables are always allocated through the frame allocator, and are
            // always exactly one frame in size. `child_base` is that frame.
            unsafe {
                self.frame_allocator
                    .deallocate(child_base, A::GRANULE_LAYOUT);
            }

            // TODO fence(all pages, 0) if attributes includes GLOBAL
            self.flush.invalidate_all();
        }

        Ok(())
    }

    fn fill(
        &mut self,
        table: &mut Table<A, marker::Mut<'_>>,
        first: u16,
        count: u16,
        va: VirtualAddress,
        physmap: &PhysMap,
        arch: &A,
    ) -> Result<(), Infallible> {
        let mut entry_virt = table.entry_address(first, physmap);

        for _ in 0..count {
            debug_assert!(
                // Safety: `entry_virt` is within the covered run, in-bounds and aligned.
                unsafe { arch.read::<A::PageTableEntry>(entry_virt) }.is_leaf(),
                "virtual address range must be mapped at page size {}",
                S::BYTES,
            );

            // Safety: `entry_virt` is within the covered run, in-bounds and aligned.
            unsafe { arch.write(entry_virt, A::PageTableEntry::VACANT) };

            entry_virt = entry_virt.add(size_of::<A::PageTableEntry>());
        }

        // TODO fence(modified pages, 0) if attributes includes GLOBAL
        self.flush
            .invalidate(Range::from_start_len(va, count as usize * S::BYTES));

        Ok(())
    }
}
