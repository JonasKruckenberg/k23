use crate::arch::{Arch, PageTableEntry};
use crate::frame_alloc::{FrameAllocator, NonContiguousFrames};
use crate::{arch, Flush, PhysicalAddress, VirtualAddress};
use bitflags::Flags;
use core::marker::PhantomData;
use core::num::NonZeroUsize;
use core::ptr::NonNull;

pub struct AddressSpace<A> {
    root_pgtable: PhysicalAddress,
    phys_offset: VirtualAddress,
    asid: usize,
    _m: PhantomData<A>,
}

impl<A> AddressSpace<A>
where
    A: Arch,
{
    pub fn new(
        frame_alloc: &mut dyn FrameAllocator,
        asid: usize,
        phys_offset: VirtualAddress,
    ) -> crate::Result<(Self, Flush<A>)> {
        let root_pgtable = frame_alloc.allocate_one_zeroed(phys_offset)?;

        let this = Self {
            asid,
            phys_offset,
            root_pgtable,
            _m: PhantomData,
        };

        Ok((this, Flush::empty(asid)))
    }

    pub fn from_active(asid: usize, phys_offset: VirtualAddress) -> (Self, Flush<A>) {
        let root_pgtable = A::get_active_pgtable(asid);
        debug_assert!(root_pgtable.as_raw() != 0);

        let this = Self {
            asid,
            phys_offset,
            root_pgtable,
            _m: PhantomData,
        };

        (this, Flush::empty(asid))
    }

    pub fn root_pgtable(&self) -> PhysicalAddress {
        self.root_pgtable
    }

    pub fn phys_offset(&self) -> VirtualAddress {
        self.phys_offset
    }

    pub fn phys_to_virt(&self, phys: PhysicalAddress) -> VirtualAddress {
        self.phys_offset.add(phys.as_raw())
    }

    pub fn map_contiguous(
        &mut self,
        frame_alloc: &mut dyn FrameAllocator,
        mut virt: VirtualAddress,
        mut phys: PhysicalAddress,
        len: NonZeroUsize,
        flags: crate::Flags,
        flush: &mut Flush<A>,
    ) -> crate::Result<()> {
        let mut remaining_bytes = len.get();
        debug_assert!(
            remaining_bytes >= arch::PAGE_SIZE,
            "address range span be at least one page"
        );
        debug_assert!(
            virt.is_aligned(arch::PAGE_SIZE),
            "virtual address must be aligned to at least 4KiB page size"
        );
        debug_assert!(
            phys.is_aligned(arch::PAGE_SIZE),
            "physical address must be aligned to at least 4KiB page size"
        );

        'outer: while remaining_bytes > 0 {
            let mut pgtable: NonNull<A::PageTableEntry> =
                self.pgtable_ptr_from_phys(self.root_pgtable);
            // log::trace!("outer (pgtable {pgtable:?})");

            for lvl in (0..A::PAGE_TABLE_LEVELS).rev() {
                let index = A::pte_index_for_level(virt, lvl);
                let pte = unsafe { pgtable.add(index).as_mut() };
                // log::trace!("[lvl{lvl}::{index} pte {:?}]", pte as *mut _);

                if !pte.is_valid() {
                    if A::can_map_at_level(virt, phys, remaining_bytes, lvl) {
                        let page_size = A::page_size_for_level(lvl);

                        // log::trace!("[lvl{lvl}::{index} pte {:?}] mapping {phys:?}..{:?} {flags:?} ", pte as *mut _, phys.add(page_size));

                        // This PTE is vacant AND we can map at this level
                        // mark this PTE as a valid leaf node pointing to the physical frame
                        pte.replace_address_and_flags(
                            phys,
                            <A::PageTableEntry as PageTableEntry>::FLAGS_VALID.union(flags.into()),
                        );

                        flush.extend_range(self.asid, virt..virt.add(page_size))?;
                        virt = virt.add(page_size);
                        phys = phys.add(page_size);
                        remaining_bytes -= page_size;
                        continue 'outer;
                    } else {
                        // The current PTE is vacant, but we couldn't map at this level (because the
                        // page size was too large, or the request wasn't sufficiently aligned or
                        // because the architecture just can't map at this level). This means
                        // we need to allocate a new sub-table and retry.
                        // allocate a new physical frame to hold the next level table and
                        // mark this PTE as a valid internal node pointing to that sub-table.
                        let frame = frame_alloc.allocate_one_zeroed(self.phys_offset)?;

                        // log::trace!("[lvl{lvl}::{index} pte {:?}] allocating sub table {frame:?}", pte as *mut _,);

                        pte.replace_address_and_flags(
                            frame,
                            <A::PageTableEntry as PageTableEntry>::FLAGS_VALID,
                        );

                        pgtable = self.pgtable_ptr_from_phys(frame);
                    }
                } else if !pte.is_leaf() {
                    // log::trace!("[lvl{lvl}::{index} pte {:?}] is sub-table => {:?}", pte as *mut _, pte.get_address_and_flags().0);

                    // This PTE is an internal node pointing to another page table
                    pgtable = self.pgtable_ptr_from_phys(pte.get_address_and_flags().0);
                } else {
                    unreachable!("Invalid state: PTE can't be valid leaf (this means {virt:?} is already mapped) {pte:?}");
                }
            }
        }

        Ok(())
    }

    pub fn map(
        &mut self,
        mut virt: VirtualAddress,
        mut iter: NonContiguousFrames,
        flags: crate::Flags,
        flush: &mut Flush<A>,
    ) -> crate::Result<()> {
        while let Some((phys, len)) = iter.next().transpose()? {
            self.map_contiguous(
                iter.alloc_mut(),
                virt,
                phys,
                NonZeroUsize::new(len.get() * arch::PAGE_SIZE).unwrap(),
                flags,
                flush,
            )?;
            virt = virt.add(len.get() * arch::PAGE_SIZE);
        }

        Ok(())
    }

    pub fn remap_contiguous(
        &mut self,
        mut virt: VirtualAddress,
        mut phys: PhysicalAddress,
        len: NonZeroUsize,
        flush: &mut Flush<A>,
    ) -> crate::Result<()> {
        let mut remaining_bytes = len.get();
        debug_assert!(
            remaining_bytes >= arch::PAGE_SIZE,
            "virtual address range must span be at least one page"
        );
        debug_assert!(
            virt.is_aligned(arch::PAGE_SIZE),
            "virtual address must be aligned to at least 4KiB page size"
        );
        debug_assert!(
            phys.is_aligned(arch::PAGE_SIZE),
            "physical address must be aligned to at least 4KiB page size"
        );

        'outer: while remaining_bytes > 0 {
            let mut pgtable = self.pgtable_ptr_from_phys(self.root_pgtable);

            for lvl in (0..A::PAGE_TABLE_LEVELS).rev() {
                let pte = unsafe {
                    let index = A::pte_index_for_level(virt, lvl);
                    pgtable.add(index).as_mut()
                };

                if pte.is_valid() && pte.is_leaf() {
                    // We reached the previously mapped leaf node that we want to edit
                    // assert that we can actually map at this level (remap requires users to remap
                    // only to equal or larger alignments, but we should make sure.
                    let page_size = A::page_size_for_level(lvl);

                    // TODO replace this with an error
                    debug_assert!(
                        A::can_map_at_level(virt, phys, remaining_bytes, lvl),
                        "remapping requires the same alignment and page size ({page_size}) but found {phys:?}, {remaining_bytes}bytes"
                    );

                    let (_old_phys, flags) = pte.get_address_and_flags();
                    pte.replace_address_and_flags(phys, flags);

                    flush.extend_range(self.asid, virt..virt.add(page_size))?;
                    virt = virt.add(page_size);
                    phys = phys.add(page_size);
                    remaining_bytes -= page_size;
                    continue 'outer;
                } else if pte.is_valid() {
                    // This PTE is an internal node pointing to another page table
                    pgtable = self.pgtable_ptr_from_phys(pte.get_address_and_flags().0);
                } else {
                    unreachable!("Invalid state: PTE cant be vacant or invalid+leaf {pte:?}");
                }
            }
        }

        Ok(())
    }

    pub fn remap(
        &mut self,
        mut virt: VirtualAddress,
        mut iter: NonContiguousFrames,
        flush: &mut Flush<A>,
    ) -> crate::Result<()> {
        while let Some((phys, len)) = iter.next().transpose()? {
            self.remap_contiguous(
                virt,
                phys,
                NonZeroUsize::new(len.get() * arch::PAGE_SIZE).unwrap(),
                flush,
            )?;
            virt = virt.add(len.get() * arch::PAGE_SIZE);
        }

        Ok(())
    }

    pub fn protect(
        &mut self,
        mut virt: VirtualAddress,
        len: NonZeroUsize,
        new_flags: crate::Flags,
        flush: &mut Flush<A>,
    ) -> crate::Result<()> {
        let mut remaining_bytes = len.get();
        debug_assert!(
            remaining_bytes >= arch::PAGE_SIZE,
            "virtual address range must span be at least one page"
        );
        debug_assert!(
            virt.is_aligned(arch::PAGE_SIZE),
            "virtual address must be aligned to at least 4KiB page size"
        );

        'outer: while remaining_bytes > 0 {
            let mut pgtable = self.pgtable_ptr_from_phys(self.root_pgtable);

            for lvl in (0..A::PAGE_TABLE_LEVELS).rev() {
                let pte = unsafe {
                    let index = A::pte_index_for_level(virt, lvl);
                    pgtable.add(index).as_mut()
                };

                if pte.is_valid() && pte.is_leaf() {
                    // We reached the previously mapped leaf node that we want to edit
                    // firstly, ensure that this operation only removes permissions never adds any
                    // and secondly mask out the old permissions replacing them with the new. This must
                    // ensure we retain any other flags in the process.
                    let rwx_mask = <A::PageTableEntry as PageTableEntry>::FLAGS_RWX;

                    let new_flags = rwx_mask.intersection(new_flags.into());
                    let (phys, old_flags) = pte.get_address_and_flags();

                    // TODO replace this with an error
                    debug_assert!(old_flags.intersection(rwx_mask).contains(new_flags));

                    pte.replace_address_and_flags(
                        phys,
                        old_flags.difference(rwx_mask).union(new_flags),
                    );

                    let page_size = A::page_size_for_level(lvl);
                    flush.extend_range(self.asid, virt..virt.add(page_size))?;
                    virt = virt.add(page_size);
                    remaining_bytes -= page_size;
                    continue 'outer;
                } else if pte.is_valid() {
                    // This PTE is an internal node pointing to another page table
                    pgtable = self.pgtable_ptr_from_phys(pte.get_address_and_flags().0);
                } else {
                    unreachable!("Invalid state: PTE cant be vacant or invalid+leaf {pte:?}");
                }
            }
        }

        Ok(())
    }

    pub fn query(
        &mut self,
        virt: VirtualAddress,
    ) -> Option<(
        PhysicalAddress,
        <A::PageTableEntry as PageTableEntry>::Flags,
    )> {
        let mut pgtable: NonNull<A::PageTableEntry> = self.pgtable_ptr_from_phys(self.root_pgtable);

        for lvl in (0..A::PAGE_TABLE_LEVELS).rev() {
            let pte = unsafe {
                let index = A::pte_index_for_level(virt, lvl);
                pgtable.add(index).as_mut()
            };

            if pte.is_valid() && pte.is_leaf() {
                return Some(pte.get_address_and_flags());
            } else if pte.is_valid() {
                // This PTE is an internal node pointing to another page table
                pgtable = self.pgtable_ptr_from_phys(pte.get_address_and_flags().0);
            } else {
                unreachable!("Invalid state: PTE cant be vacant or invalid+leaf {pte:?}");
            }
        }

        None
    }

    /// Activate this address space
    ///
    /// # Safety
    ///
    /// This will invalidate pointers if not used carefully
    pub unsafe fn activate(&self) {
        A::activate_pgtable(self.asid, self.root_pgtable)
    }

    fn pgtable_ptr_from_phys(&self, phys: PhysicalAddress) -> NonNull<A::PageTableEntry> {
        NonNull::new(self.phys_offset.add(phys.as_raw()).as_raw() as *mut _).unwrap()
    }
}
