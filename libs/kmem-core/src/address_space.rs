// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ops::Range;

use fallible_iterator::FallibleIterator;

use crate::arch::PageTableEntry;
use crate::frame_alloc::{AllocError, FrameAllocator};
use crate::table::{marker, Table};
use crate::visitors::lookup::LookupVisitor;
use crate::visitors::map::MapVisitor;
use crate::visitors::remap::RemapVisitor;
use crate::visitors::set_attributes::SetAttributesVisitor;
use crate::visitors::unmap::UnmapVisitor;
use crate::{
    utils, AddressRangeExt, Arch, Flush, MemoryAttributes, PageTableLevel,
    PhysicalAddress, VirtualAddress,
};

pub trait Visit<A: Arch> {
    type Error;

    /// Visit a (_any_) page table entry, regardless of type.
    ///
    /// # Errors
    ///
    /// The implementation may return `Err(Self::Error)` to signal the walker to terminate with the
    /// given error.
    #[allow(unused_variables, reason = "formatting")]
    fn visit_entry(
        &mut self,
        entry: A::PageTableEntry,
        level: &'static PageTableLevel,
        range: Range<VirtualAddress>,
        arch: &A,
    ) -> Result<(), Self::Error>;
}

pub trait VisitMut<A: Arch> {
    type Error;

    /// Visit a (_any_) page table entry, regardless of type.
    ///
    /// The implementation may update the entry in-place by mutating the provided mutable reference.
    ///
    /// # Errors
    ///
    /// The implementation may return `Err(Self::Error)` to signal the walker to terminate with the
    /// given error.
    #[allow(unused_variables, reason = "formatting")]
    fn visit_entry(
        &mut self,
        entry: &mut A::PageTableEntry,
        level: &'static PageTableLevel,
        range: Range<VirtualAddress>,
        arch: &A,
    ) -> Result<(), Self::Error>;

    /// Called _after_ a subtable has been visited.
    ///
    /// This is currently only used by the `unmap` routine to clean up empty page tables.
    ///
    /// # Errors
    ///
    /// The implementation may return `Err(Self::Error)` to signal the walker to terminate with the
    /// given error.
    #[allow(unused_variables, reason = "formatting")]
    fn after_subtable(
        &mut self,
        entry: &mut A::PageTableEntry,
        table: Table<A, marker::Mut<'_>>,
        level: &'static PageTableLevel,
        range: Range<VirtualAddress>,
        arch: &A,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct AddressSpace<A: Arch = crate::arch::DefaultArchForArchitecture> {
    arch: A,
    root_page_table: Table<A, marker::Owned>,
}

impl<A: Arch> AddressSpace<A> {
    /// Constructs a new `AddressSpace` with a freshly allocated root page table.
    ///
    /// # Errors
    ///
    /// Returns `Err(AllocError)` when allocating the root page table fails.
    pub fn new<F>(arch: A, frame_allocator: F, flush: &mut Flush) -> Result<Self, AllocError>
    where
        F: FrameAllocator,
    {
        let root_page_table =
            frame_allocator.allocate_contiguous_zeroed(arch.memory_mode().page_layout(), &arch)?;

        // Safety: we have just allocated the frame above, we own it (1., 2.) and since were creating a new table
        // depth=0 is also correct (3.).
        let root_page_table = unsafe { Table::from_raw_parts(root_page_table, 0) };
        let me = Self::from_raw_parts(arch, root_page_table);

        flush.invalidate_all();

        Ok(me)
    }

    /// Constructs a new `AddressSpace` from its raw components: architecture-specific data and the root table.
    pub fn from_raw_parts(arch: A, root_page_table: Table<A, marker::Owned>) -> Self {
        Self {
            arch,
            root_page_table,
        }
    }

    /// Decomposes an `AddressSpace` into its raw components: architecture-specific data and the root table.
    pub fn into_raw_parts(self) -> (A, Table<A, marker::Owned>) {
        (self.arch, self.root_page_table)
    }

    /// Returns an immutable reference to the architecture-specific data.
    pub fn arch(&self) -> &A {
        &self.arch
    }

    /// Returns a mutable reference to the architecture-specific data.
    pub fn arch_mut(&mut self) -> &mut A {
        &mut self.arch
    }

    /// Activate the address space on this CPU (set this CPUs page table).
    ///
    /// # Safety
    ///
    /// After this method returns, all pointers become dangling and as such any access through
    /// pre-existing pointers is Undefined Behaviour. This includes implicit references by the CPU
    /// such as the instruction pointer.
    pub unsafe fn activate(&self) {
        // Safety: ensured by caller
        unsafe {
            self.arch.set_active_table(self.root_page_table.address());
        }
    }

    /// Return the corresponding [`PhysicalAddress`] and [`AccessRules`] for the given
    /// [`VirtualAddress`] if mapped. The returned [`PageTableLevel`] described the page table level
    /// at which the mapping was found.
    pub fn lookup(
        &self,
        virt: VirtualAddress,
    ) -> Option<(PhysicalAddress, MemoryAttributes, &'static PageTableLevel)> {
        let mut v = LookupVisitor::new();

        // Safety: LookupVisitor::Error is Infallible
        unsafe {
            self.visit(
                Range::from_start_len(virt, self.arch.memory_mode().page_size()),
                &mut v,
            )
            .unwrap_unchecked();
        }

        v.into_result()
    }

    /// Maps the virtual address range `virt` to potentially discontiguous regions of physical memory
    /// with the specified memory attributes. (TODO why is this preferable?)
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
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the mapping cannot be established and the address space remains
    /// unaltered.
    pub unsafe fn map<F>(
        &mut self,
        mut virt: Range<VirtualAddress>,
        mut phys: impl FallibleIterator<Item = Range<PhysicalAddress>, Error = AllocError>,
        attributes: MemoryAttributes,
        frame_allocator: F,
        flush: &mut Flush,
    ) -> Result<(), AllocError>
    where
        F: FrameAllocator,
    {
        while let Some(phys) = phys.next()? {
            debug_assert!(!virt.is_empty());

            // Safety: ensured by caller (1.,2.) and the frame allocator always returning aligned chunks.
            unsafe {
                self.map_contiguous(
                    Range::from_start_len(virt.start, phys.len()),
                    phys.start,
                    attributes,
                    frame_allocator.by_ref(),
                    flush,
                )?;
            }

            virt.start = virt.start.add(phys.len());
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
    /// 2. `virt` must be aligned to at least the smallest architecture block size.
    /// 3. `phys` must be aligned to at least the smallest architecture block size.
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the mapping cannot be established and the address space remains
    /// unaltered.
    pub unsafe fn map_contiguous<F>(
        &mut self,
        virt: Range<VirtualAddress>,
        phys: PhysicalAddress,
        attributes: MemoryAttributes,
        frame_allocator: F,
        flush: &mut Flush,
    ) -> Result<(), AllocError>
    where
        F: FrameAllocator,
    {
        debug_assert!(
            virt.len() >= self.arch.memory_mode().page_size(),
            "address range span be at least one page"
        );
        debug_assert!(
            virt.start
                .is_aligned_to(self.arch.memory_mode().page_size()),
            "virtual address {} must be aligned to at least page size {}",
            virt.start,
            self.arch.memory_mode().page_size()
        );
        debug_assert!(
            phys.is_aligned_to(self.arch.memory_mode().page_size()),
            "physical address {phys} must be aligned to at least page size {}",
            self.arch.memory_mode().page_size()
        );

        let mut v = MapVisitor::new(
            phys,
            attributes,
            frame_allocator,
            self.arch.memory_mode().page_layout(),
            flush,
        );

        self.visit_mut(virt, &mut v)?;

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
        phys: PhysicalAddress,
        flush: &mut Flush,
    ) {
        debug_assert!(
            virt.len() >= self.arch.memory_mode().page_size(),
            "address range span be at least one page"
        );
        debug_assert!(
            virt.start
                .is_aligned_to(self.arch.memory_mode().page_size()),
            "virtual address {} must be aligned to at least page size {}",
            virt.start,
            self.arch.memory_mode().page_size()
        );
        debug_assert!(
            phys.is_aligned_to(self.arch.memory_mode().page_size()),
            "physical address {phys} must be aligned to at least page size {}",
            self.arch.memory_mode().page_size()
        );

        let mut v = RemapVisitor::new(phys, flush);

        // Safety: RemapVisitor::Error is Infallible
        unsafe {
            self.visit_mut(virt, &mut v).unwrap_unchecked();
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
            virt.len() >= self.arch.memory_mode().page_size(),
            "address range span be at least one page"
        );
        debug_assert!(
            virt.start
                .is_aligned_to(self.arch.memory_mode().page_size()),
            "virtual address {} must be aligned to at least page size {}",
            virt.start,
            self.arch.memory_mode().page_size()
        );

        let mut v = SetAttributesVisitor::new(attributes, flush);

        // Safety: SetAttributesVisitor::Error is Infallible
        unsafe {
            self.visit_mut(virt, &mut v).unwrap_unchecked();
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
    pub unsafe fn unmap<F>(
        &mut self,
        virt: Range<VirtualAddress>,
        frame_allocator: F,
        flush: &mut Flush,
    ) where
        F: FrameAllocator,
    {
        debug_assert!(
            virt.len() >= self.arch.memory_mode().page_size(),
            "address range span be at least one page"
        );
        debug_assert!(
            virt.start
                .is_aligned_to(self.arch.memory_mode().page_size()),
            "virtual address {} must be aligned to at least page size {}",
            virt.start,
            self.arch.memory_mode().page_size()
        );

        let mut v = UnmapVisitor::new(
            frame_allocator,
            self.arch.memory_mode().page_layout(),
            flush,
        );

        // Safety: UnmapVisitor::Error is Infallible
        unsafe {
            self.visit_mut(virt, &mut v).unwrap_unchecked();
        }
    }

    /// Visit all entries in the virtual address range `virt`.
    ///
    /// # Errors
    ///
    /// Forwards errors returned by the visitor implementation.
    pub fn visit<V>(&self, range: Range<VirtualAddress>, visitor: &mut V) -> Result<(), V::Error>
    where
        V: Visit<A>,
    {
        Self::visit_inner(self.root_page_table.borrow(), range, visitor, &self.arch)
    }

    fn visit_inner<V>(
        table: Table<A, marker::Immut<'_>>,
        range: Range<VirtualAddress>,
        visitor: &mut V,
        arch: &A,
    ) -> Result<(), V::Error>
    where
        V: Visit<A>,
    {
        let level = table.level(arch);

        let entries = utils::page_table_entries_for(range, table.level(arch), arch.memory_mode());
        for (entry_index, range) in entries {
            // Safety: `page_table_entries_for` returns only in-bound indices.
            let entry = unsafe { table.get(entry_index, arch) };

            if entry.is_table() {
                // Safety: We checked the entry is a table above (1.) know the depth is correct (just increased by one) (3.)
                // and also know creating an immutable reference is always safe given `table` is immutable too. (2.)
                let subtable: Table<A, marker::Immut<'_>> =
                    unsafe { Table::from_raw_parts(entry.address(), table.depth() + 1) };

                Self::visit_inner(subtable, range.clone(), visitor, arch)?;
            } else {
                visitor.visit_entry(entry, level, range.clone(), arch)?;
            }
        }

        Ok(())
    }

    /// Visit all entries in the virtual address range `virt` mutably.
    ///
    /// # Errors
    ///
    /// Forwards errors returned by the visitor implementation.
    pub fn visit_mut<V>(
        &mut self,
        range: Range<VirtualAddress>,
        visitor: &mut V,
    ) -> Result<(), V::Error>
    where
        V: VisitMut<A>,
    {
        Self::visit_mut_inner(
            self.root_page_table.borrow_mut(),
            range,
            visitor,
            &self.arch,
        )
    }

    fn visit_mut_inner<V>(
        mut table: Table<A, marker::Mut<'_>>,
        range: Range<VirtualAddress>,
        visitor: &mut V,
        arch: &A,
    ) -> Result<(), V::Error>
    where
        V: VisitMut<A>,
    {
        let level = table.level(arch);

        let entries = utils::page_table_entries_for(range, table.level(arch), arch.memory_mode());
        for (entry_index, range) in entries {
            // Safety: `page_table_entries_for` returns only in-bound indices.
            let mut entry = unsafe { table.get(entry_index, arch) };

            visitor.visit_entry(&mut entry, level, range.clone(), arch)?;

            if entry.is_table() {
                debug_assert!(((table.depth() + 1) as usize) < arch.memory_mode().levels().len());

                // Safety: We checked the entry is a table above (1.) know the depth is correct (just increased by one) (3.).
                // Creating a mutable subtable here is also safe because it never escapes this block, and we are careful not
                // to violate aliasing rules. (2.)
                let mut subtable =
                    unsafe { Table::from_raw_parts(entry.address(), table.depth() + 1) };

                Self::visit_mut_inner(subtable.reborrow_mut(), range.clone(), visitor, arch)?;

                visitor.after_subtable(&mut entry, subtable, level, range, arch)?;
            }

            // Safety: `page_table_entries_for` returns only in-bound indices.
            unsafe {
                table.set(entry_index, entry, arch);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use riscv::satp;

    use crate::address_range::AddressRangeExt;
    use crate::arch::riscv64::Riscv64;
    use crate::frame_alloc::FrameAllocator;
    use crate::test_utils::setup_aspace_and_alloc;
    use crate::{Arch, Flush, MemoryAttributes, VirtualAddress, WriteOrExecute};

    #[test]
    fn map() {
        let (mut aspace, frame_alloc) =
            setup_aspace_and_alloc(Riscv64::new(1, satp::Mode::Sv39), [0x6000]);

        let frame = frame_alloc
            .allocate_contiguous(aspace.arch().memory_mode().page_layout())
            .unwrap();

        let page = Range::from_start_len(
            VirtualAddress::new(0x7000),
            aspace.arch().memory_mode().page_size(),
        );

        let mut flush = Flush::new();
        unsafe {
            aspace
                .map_contiguous(
                    page.clone(),
                    frame,
                    MemoryAttributes::new().with(MemoryAttributes::READ, true),
                    frame_alloc.by_ref(),
                    &mut flush,
                )
                .unwrap();
        }
        flush.flush(aspace.arch());

        let (phys, attrs, lvl) = aspace.lookup(page.start).unwrap();

        assert_eq!(phys, frame);
        assert_eq!(attrs.allows_read(), true);
        assert_eq!(attrs.allows_write(), false);
        assert_eq!(attrs.allows_execution(), false);
        assert_eq!(lvl.page_size(), 4096);
    }

    #[test]
    fn remap() {
        let (mut aspace, frame_alloc) =
            setup_aspace_and_alloc(Riscv64::new(1, satp::Mode::Sv39), [0x7000]);

        let frame = frame_alloc
            .allocate_contiguous(aspace.arch().memory_mode().page_layout())
            .unwrap();

        let page = Range::from_start_len(
            VirtualAddress::new(0x7000),
            aspace.arch().memory_mode().page_size(),
        );

        let mut flush = Flush::new();
        unsafe {
            aspace
                .map_contiguous(
                    page.clone(),
                    frame,
                    MemoryAttributes::new().with(MemoryAttributes::READ, true),
                    frame_alloc.by_ref(),
                    &mut flush,
                )
                .unwrap();
        }
        flush.flush(aspace.arch());

        let (phys, attrs, lvl) = aspace.lookup(page.start).unwrap();

        assert_eq!(phys, frame);
        assert_eq!(attrs.allows_read(), true);
        assert_eq!(attrs.allows_write(), false);
        assert_eq!(attrs.allows_execution(), false);
        assert_eq!(lvl.page_size(), 4096);

        // ===== the actual remap part =====

        let new_frame = frame_alloc
            .allocate_contiguous(aspace.arch().memory_mode().page_layout())
            .unwrap();

        let mut flush = Flush::new();
        unsafe {
            aspace.remap_contiguous(page.clone(), new_frame, &mut flush);
        }
        flush.flush(aspace.arch());

        let (phys, attrs, lvl) = aspace.lookup(page.start).unwrap();

        assert_eq!(phys, new_frame);
        assert_eq!(attrs.allows_read(), true);
        assert_eq!(attrs.allows_write(), false);
        assert_eq!(attrs.allows_execution(), false);
        assert_eq!(lvl.page_size(), 4096);
    }

    #[test]
    fn set_attributes() {
        let (mut aspace, frame_alloc) =
            setup_aspace_and_alloc(Riscv64::new(1, satp::Mode::Sv39), [0x7000]);

        let frame = frame_alloc
            .allocate_contiguous(aspace.arch().memory_mode().page_layout())
            .unwrap();

        let page = Range::from_start_len(
            VirtualAddress::new(0x7000),
            aspace.arch().memory_mode().page_size(),
        );

        let mut flush = Flush::new();
        unsafe {
            aspace
                .map_contiguous(
                    page.clone(),
                    frame,
                    MemoryAttributes::new().with(MemoryAttributes::READ, true),
                    frame_alloc.by_ref(),
                    &mut flush,
                )
                .unwrap();
        }
        flush.flush(aspace.arch());

        let (phys, attrs, lvl) = aspace.lookup(page.start).unwrap();

        assert_eq!(phys, frame);
        assert_eq!(attrs.allows_read(), true);
        assert_eq!(attrs.allows_write(), false);
        assert_eq!(attrs.allows_execution(), false);
        assert_eq!(lvl.page_size(), 4096);

        // ===== the actual remap part =====

        let mut flush = Flush::new();
        unsafe {
            aspace.set_attributes(
                page.clone(),
                MemoryAttributes::new()
                    .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Execute),
                &mut flush,
            );
        }
        flush.flush(aspace.arch());

        let (phys, attrs, lvl) = aspace.lookup(page.start).unwrap();

        assert_eq!(phys, frame);
        assert_eq!(attrs.allows_read(), false);
        assert_eq!(attrs.allows_write(), false);
        assert_eq!(attrs.allows_execution(), true);
        assert_eq!(lvl.page_size(), 4096);
    }

    #[ignore = "BootstrapAllocator can't free"]
    #[test]
    fn unmap() {
        let (mut aspace, frame_alloc) =
            setup_aspace_and_alloc(Riscv64::new(1, satp::Mode::Sv39), [0x6000]);

        let frame = frame_alloc
            .allocate_contiguous(aspace.arch().memory_mode().page_layout())
            .unwrap();

        let page = Range::from_start_len(
            VirtualAddress::new(0x7000),
            aspace.arch().memory_mode().page_size(),
        );

        let mut flush = Flush::new();
        unsafe {
            aspace
                .map_contiguous(
                    page.clone(),
                    frame,
                    MemoryAttributes::new().with(MemoryAttributes::READ, true),
                    frame_alloc.by_ref(),
                    &mut flush,
                )
                .unwrap();
        }
        flush.flush(aspace.arch());

        let (phys, attrs, lvl) = aspace.lookup(page.start).unwrap();

        assert_eq!(phys, frame);
        assert_eq!(attrs.allows_read(), true);
        assert_eq!(attrs.allows_write(), false);
        assert_eq!(attrs.allows_execution(), false);
        assert_eq!(lvl.page_size(), 4096);

        // ===== the actual unmapping part =====

        let mut flush = Flush::new();
        unsafe {
            aspace.unmap(page.clone(), frame_alloc.by_ref(), &mut flush);
        }
        flush.flush(aspace.arch());

        assert!(aspace.lookup(page.start).is_none());
    }
}
