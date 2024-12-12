use crate::frame_alloc::{FrameAllocator, NonContiguousFrames};
use crate::{arch, Flush, PhysicalAddress, VirtualAddress};
use core::fmt;
use core::fmt::Formatter;
use core::num::NonZeroUsize;
use core::ptr::NonNull;

pub struct AddressSpace {
    root_pgtable: PhysicalAddress,
    phys_offset: VirtualAddress,
    asid: usize,
}

impl AddressSpace {
    pub fn new(
        frame_alloc: &mut dyn FrameAllocator,
        asid: usize,
        phys_offset: VirtualAddress,
    ) -> crate::Result<(Self, Flush)> {
        let root_pgtable = frame_alloc.allocate_one_zeroed(phys_offset)?;

        let this = Self {
            asid,
            phys_offset,
            root_pgtable,
        };

        Ok((this, Flush::empty(asid)))
    }

    pub fn from_active(asid: usize, phys_offset: VirtualAddress) -> (Self, Flush) {
        let root_pgtable = arch::get_active_pgtable(asid);
        debug_assert!(root_pgtable.as_raw() != 0);

        let this = Self {
            asid,
            phys_offset,
            root_pgtable,
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

    pub fn asid(&self) -> usize {
        self.asid
    }

    pub fn map_contiguous(
        &mut self,
        frame_alloc: &mut dyn FrameAllocator,
        mut virt: VirtualAddress,
        mut phys: PhysicalAddress,
        len: NonZeroUsize,
        flags: crate::Flags,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        let mut remaining_bytes = len.get();
        debug_assert!(
            remaining_bytes >= arch::PAGE_SIZE,
            "address range span be at least one page"
        );
        debug_assert!(
            virt.is_aligned(arch::PAGE_SIZE),
            "virtual address must be aligned to at least 4KiB page size {virt:?}"
        );
        debug_assert!(
            phys.is_aligned(arch::PAGE_SIZE),
            "physical address must be aligned to at least 4KiB page size {phys:?}"
        );

        'outer: while remaining_bytes > 0 {
            let mut pgtable: NonNull<arch::PageTableEntry> =
                self.pgtable_ptr_from_phys(self.root_pgtable);
            // log::trace!("outer (pgtable {pgtable:?})");

            for lvl in (0..arch::PAGE_TABLE_LEVELS).rev() {
                let index = arch::pte_index_for_level(virt, lvl);
                let pte = unsafe { pgtable.add(index).as_mut() };

                // log::trace!("[lvl{lvl}::{index} pte {:?}]", pte as *mut _);

                if !pte.is_valid() {
                    if arch::can_map_at_level(virt, phys, remaining_bytes, lvl) {
                        let page_size = arch::page_size_for_level(lvl);

                        // log::trace!("[lvl{lvl}::{index} pte {:?}] mapping {phys:?}..{:?} {flags:?} ", pte as *mut _, phys.add(page_size));

                        // This PTE is vacant AND we can map at this level
                        // mark this PTE as a valid leaf node pointing to the physical frame
                        pte.replace_address_and_flags(
                            phys,
                            arch::PTE_FLAGS_VALID.union(flags.into()),
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

                        pte.replace_address_and_flags(frame, arch::PTE_FLAGS_VALID);

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
        flush: &mut Flush,
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
        flush: &mut Flush,
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

            for lvl in (0..arch::PAGE_TABLE_LEVELS).rev() {
                let pte = unsafe {
                    let index = arch::pte_index_for_level(virt, lvl);
                    pgtable.add(index).as_mut()
                };

                if pte.is_valid() && pte.is_leaf() {
                    // We reached the previously mapped leaf node that we want to edit
                    // assert that we can actually map at this level (remap requires users to remap
                    // only to equal or larger alignments, but we should make sure.
                    let page_size = arch::page_size_for_level(lvl);

                    // TODO replace this with an error
                    debug_assert!(
                        arch::can_map_at_level(virt, phys, remaining_bytes, lvl),
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
        flush: &mut Flush,
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
        flush: &mut Flush,
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

            for lvl in (0..arch::PAGE_TABLE_LEVELS).rev() {
                let pte = unsafe {
                    let index = arch::pte_index_for_level(virt, lvl);
                    pgtable.add(index).as_mut()
                };

                if pte.is_valid() && pte.is_leaf() {
                    // We reached the previously mapped leaf node that we want to edit
                    // firstly, ensure that this operation only removes permissions never adds any
                    // and secondly mask out the old permissions replacing them with the new. This must
                    // ensure we retain any other flags in the process.
                    let rwx_mask = arch::PTE_FLAGS_RWX_MASK;

                    let new_flags = rwx_mask.intersection(new_flags.into());
                    let (phys, old_flags) = pte.get_address_and_flags();

                    // TODO replace this with an error
                    debug_assert!(old_flags.intersection(rwx_mask).contains(new_flags));

                    pte.replace_address_and_flags(
                        phys,
                        old_flags.difference(rwx_mask).union(new_flags),
                    );

                    let page_size = arch::page_size_for_level(lvl);
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

    pub fn unmap(
        &mut self,
        frame_alloc: &mut dyn FrameAllocator,
        mut virt: VirtualAddress,
        len: NonZeroUsize,
        flush: &mut Flush,
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

        while remaining_bytes > 0 {
            self.unmap_inner(
                self.pgtable_ptr_from_phys(self.root_pgtable),
                frame_alloc,
                &mut virt,
                &mut remaining_bytes,
                arch::PAGE_TABLE_LEVELS - 1,
                flush,
            )?;
        }

        Ok(())
    }

    fn unmap_inner(
        &mut self,
        pgtable: NonNull<arch::PageTableEntry>,
        frame_alloc: &mut dyn FrameAllocator,
        virt: &mut VirtualAddress,
        remaining_bytes: &mut usize,
        lvl: usize,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        let index = arch::pte_index_for_level(*virt, lvl);
        let pte = unsafe { pgtable.add(index).as_mut() };

        if pte.is_valid() && pte.is_leaf() {
            let page_size = arch::page_size_for_level(lvl);
            let frame = pte.get_address_and_flags().0;
            frame_alloc.deallocate(
                frame,
                NonZeroUsize::new(page_size / arch::PAGE_SIZE).unwrap(),
            )?;
            pte.clear();

            flush.extend_range(self.asid, *virt..virt.add(page_size))?;
            *virt = virt.add(page_size);
            *remaining_bytes -= page_size;
        } else if pte.is_valid() {
            // This PTE is an internal node pointing to another page table
            let pgtable = self.pgtable_ptr_from_phys(pte.get_address_and_flags().0);
            self.unmap_inner(pgtable, frame_alloc, virt, remaining_bytes, lvl - 1, flush)?;

            let is_still_populated = (0..arch::PAGE_TABLE_ENTRIES)
                .any(|index| unsafe { pgtable.add(index).as_ref().is_valid() });

            if !is_still_populated {
                let frame = pte.get_address_and_flags().0;
                frame_alloc.deallocate(frame, NonZeroUsize::new(1).unwrap())?;
                pte.clear();
            }
        }

        Ok(())
    }

    pub fn query(&mut self, virt: VirtualAddress) -> Option<(PhysicalAddress, crate::Flags)> {
        let mut pgtable: NonNull<arch::PageTableEntry> =
            self.pgtable_ptr_from_phys(self.root_pgtable);

        for lvl in (0..arch::PAGE_TABLE_LEVELS).rev() {
            let pte = unsafe {
                let index = arch::pte_index_for_level(virt, lvl);
                pgtable.add(index).as_mut()
            };

            if pte.is_valid() && pte.is_leaf() {
                let (addr, flags) = pte.get_address_and_flags();
                return Some((addr, flags.into()));
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
        arch::activate_pgtable(self.asid, self.root_pgtable)
    }

    fn pgtable_ptr_from_phys(&self, phys: PhysicalAddress) -> NonNull<arch::PageTableEntry> {
        NonNull::new(self.phys_offset.add(phys.as_raw()).as_raw() as *mut _).unwrap()
    }
}

impl fmt::Display for AddressSpace {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fn fmt_table(
            f: &mut Formatter<'_>,
            aspace: &AddressSpace,
            pgtable: NonNull<arch::PageTableEntry>,
            acc: VirtualAddress,
            lvl: usize,
        ) -> fmt::Result {
            let padding = match lvl {
                0 => 8,
                1 => 4,
                _ => 0,
            };

            for index in 0..arch::PAGE_TABLE_ENTRIES {
                let pte = unsafe { pgtable.add(index).as_mut() };
                let virt = VirtualAddress(acc.as_raw() | virt_from_index(lvl, index).as_raw());
                let (address, flags) = pte.get_address_and_flags();

                if pte.is_valid() && pte.is_leaf() {
                    writeln!(
                        f,
                        "{:^padding$}{}:{index:<3} is a leaf {} => {} {:?}",
                        "", lvl, virt, address, flags
                    )?;
                } else if pte.is_valid() {
                    writeln!(f, "{:^padding$}{}:{index} is a table node", "", lvl)?;
                    let (address, _) = pte.get_address_and_flags();
                    let pgtable = aspace.pgtable_ptr_from_phys(address);
                    fmt_table(f, aspace, pgtable, virt, lvl - 1)?
                }
            }

            Ok(())
        }

        fmt_table(
            f,
            self,
            self.pgtable_ptr_from_phys(self.root_pgtable),
            VirtualAddress::default(),
            arch::PAGE_TABLE_LEVELS - 1,
        )
    }
}

#[allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]
fn virt_from_index(lvl: usize, index: usize) -> VirtualAddress {
    let raw = ((index & (arch::PAGE_TABLE_ENTRIES - 1))
        << (lvl * arch::PAGE_ENTRY_SHIFT + arch::PAGE_SHIFT)) as isize;

    let shift = size_of::<usize>() as u32 * 8 - (arch::VIRT_ADDR_BITS + 1);
    VirtualAddress(raw.wrapping_shl(shift).wrapping_shr(shift) as usize)
}
