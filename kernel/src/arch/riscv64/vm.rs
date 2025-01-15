// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::vm::flush::Flush;
use crate::vm::frame_alloc::Frame;
use crate::vm::{frame_alloc, PhysicalAddress, VirtualAddress};
use alloc::vec;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::fmt;
use core::num::NonZeroUsize;
use core::ptr::NonNull;
use core::range::Range;
use riscv::satp;
use riscv::sbi::rfence::sfence_vma_asid;
use static_assertions::const_assert_eq;

pub const DEFAULT_ASID: usize = 0;

/// Virtual address where the kernel address space starts.
///
///
pub const KERNEL_ASPACE_BASE: VirtualAddress = VirtualAddress::new(0xffffffc000000000).unwrap();
pub const KERNEL_ASPACE_SIZE: usize = 1 << VIRT_ADDR_BITS;
const_assert_eq!(KERNEL_ASPACE_BASE.get(), CANONICAL_ADDRESS_MASK);
const_assert_eq!(KERNEL_ASPACE_SIZE - 1, !CANONICAL_ADDRESS_MASK);

/// Virtual address where the user address space starts.
///
/// The first 2MiB are reserved for catching null pointer dereferences, but this might
/// change in the future if we decide that the null-checking performed by the WASM runtime
/// is sufficiently robust.
pub const USER_ASPACE_BASE: VirtualAddress = VirtualAddress::new(0x0000000000200000).unwrap();
pub const USER_ASPACE_SIZE: usize = (1 << VIRT_ADDR_BITS) - USER_ASPACE_BASE.get();

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = (PAGE_SIZE - 1).count_ones() as usize;

pub const VIRT_ADDR_BITS: u32 = 38;
/// Canonical addresses are addresses where the tops bits (`VIRT_ADDR_BITS` to 63)
/// are all either 0 or 1.
pub const CANONICAL_ADDRESS_MASK: usize = !((1 << (VIRT_ADDR_BITS)) - 1);
const_assert_eq!(CANONICAL_ADDRESS_MASK, 0xffffffc000000000);

/// The number of page table entries in one table
pub const PAGE_TABLE_ENTRIES: usize = 512;
pub const PAGE_TABLE_LEVELS: usize = 3; // L0, L1, L2 Sv39
pub const PAGE_ENTRY_SHIFT: usize = (PAGE_TABLE_ENTRIES - 1).count_ones() as usize;
/// On `RiscV` targets the page table entry's physical address bits are shifted 2 bits to the right.
const PTE_PPN_SHIFT: usize = 2;

/// Return whether the given virtual address is in the kernel address space.
pub const fn is_kernel_address(virt: VirtualAddress) -> bool {
    virt.get() >= KERNEL_ASPACE_BASE.get()
        && virt.checked_sub_addr(KERNEL_ASPACE_BASE).unwrap() < KERNEL_ASPACE_SIZE
}

/// Return the page size for the given page table level.
///
/// # Panics
///
/// Panics if the provided level is `>= PAGE_TABLE_LEVELS`.
pub fn page_size_for_level(level: usize) -> usize {
    assert!(level < PAGE_TABLE_LEVELS);
    let page_size = 1 << (PAGE_SHIFT + level * PAGE_ENTRY_SHIFT);
    debug_assert!(page_size == 4096 || page_size == 2097152 || page_size == 1073741824);
    page_size
}

/// Parse the `level`nth page table entry index from the given virtual address.
///
/// # Panics
///
/// Panics if the provided level is `>= PAGE_TABLE_LEVELS`.
pub fn pte_index_for_level(virt: VirtualAddress, lvl: usize) -> usize {
    assert!(lvl < PAGE_TABLE_LEVELS);
    let index = (virt.get() >> (PAGE_SHIFT + lvl * PAGE_ENTRY_SHIFT)) & (PAGE_TABLE_ENTRIES - 1);
    debug_assert!(index < PAGE_TABLE_ENTRIES);

    index
}

/// Return whether the combination of `virt`,`phys`, and `remaining_bytes` can be mapped at the given `level`.
///
/// This is the case when both the virtual and physical address are aligned to the page size at this level
/// AND the remaining size is at least the page size.
pub fn can_map_at_level(
    virt: VirtualAddress,
    phys: PhysicalAddress,
    remaining_bytes: usize,
    lvl: usize,
) -> bool {
    let page_size = page_size_for_level(lvl);
    virt.is_aligned_to(page_size) && phys.is_aligned_to(page_size) && remaining_bytes >= page_size
}

/// Invalidate address translation caches for the given `address_range` in the given `address_space`.
///
/// # Errors
///
/// Should return an error if the underlying operation failed and the caches could not be invalidated.
pub fn invalidate_range(asid: usize, address_range: Range<VirtualAddress>) -> crate::Result<()> {
    let base_addr = address_range.start.get();
    let size = address_range
        .end
        .checked_sub_addr(address_range.start)
        .unwrap();
    sfence_vma_asid(0, usize::MAX, base_addr, size, asid)?;
    Ok(())
}

pub struct AddressSpace {
    root_pgtable: PhysicalAddress,
    wired_frames: Vec<Frame>,
    asid: usize,
}

impl crate::vm::ArchAddressSpace for AddressSpace {
    type Flags = PTEFlags;

    fn new(asid: usize) -> crate::Result<(Self, Flush)>
    where
        Self: Sized,
    {
        let root_pgtable = frame_alloc::alloc_one_zeroed()?;

        let this = Self {
            asid,
            root_pgtable: root_pgtable.addr(),
            wired_frames: vec![root_pgtable],
        };

        #[allow(tail_expr_drop_order)]
        Ok((this, Flush::empty(asid)))
    }

    fn from_active(asid: usize) -> (Self, Flush)
    where
        Self: Sized,
    {
        let satp = satp::read();
        assert_eq!(satp.asid(), asid);
        let root_pgtable = PhysicalAddress::new(satp.ppn() << 12);
        debug_assert!(root_pgtable.get() != 0);

        let this = Self {
            asid,
            root_pgtable,
            wired_frames: vec![],
        };

        #[allow(tail_expr_drop_order)]
        (this, Flush::empty(asid))
    }

    unsafe fn map_contiguous(
        &mut self,
        mut virt: VirtualAddress,
        mut phys: PhysicalAddress,
        len: NonZeroUsize,
        flags: Self::Flags,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        let mut remaining_bytes = len.get();
        debug_assert!(
            remaining_bytes >= PAGE_SIZE,
            "address range span be at least one page"
        );
        debug_assert!(
            virt.is_aligned_to(PAGE_SIZE),
            "virtual address must be aligned to at least 4KiB page size {virt:?}"
        );
        debug_assert!(
            phys.is_aligned_to(PAGE_SIZE),
            "physical address must be aligned to at least 4KiB page size {phys:?}"
        );

        // To map out contiguous chunk of physical memory into the virtual address space efficiently
        // we'll attempt to map as much of the chunk using as large of a page size as possible.
        //
        // We'll follow the page table down starting at the root page table entry (PTE) and check at
        // every level if we can map there. This is dictated by the combination of virtual and
        // physical address alignment as well as chunk size. If we can map at the current level
        // well subtract the page size from `remaining_bytes`, advance the current virtual and physical
        // addresses by the page size and repeat the process until there are no more bytes to map.
        //
        // IF we can't map at a given level, we'll either allocate a new PTE or follow and existing PTE
        // to the next level (and therefore smaller page size) until we reach a level that we can map at.
        // Note that, because we require a minimum alignment and size of PAGE_SIZE, we will always be
        // able to map a chunk using level 0 pages.
        //
        // In effect this algorithm will map the start and ends of chunks using smaller page sizes
        // while mapping the vast majority of the middle of a chunk using larger page sizes.
        'outer: while remaining_bytes > 0 {
            let mut pgtable: NonNull<PageTableEntry> =
                self.pgtable_ptr_from_phys(self.root_pgtable);

            for lvl in (0..PAGE_TABLE_LEVELS).rev() {
                let index = pte_index_for_level(virt, lvl);
                let pte = unsafe { pgtable.add(index).as_mut() };

                if !pte.is_valid() {
                    // If the PTE is invalid that means we reached a vacant slot to map into.
                    //
                    // First, lets check if we can map at this level of the page table given our
                    // current virtual and physical address as well as the number of remaining bytes.
                    if can_map_at_level(virt, phys, remaining_bytes, lvl) {
                        let page_size = page_size_for_level(lvl);

                        // This PTE is vacant AND we can map at this level
                        // mark this PTE as a valid leaf node pointing to the physical frame
                        pte.replace_address_and_flags(phys, PTEFlags::VALID | flags);

                        flush.extend_range(
                            self.asid,
                            Range::from(virt..virt.checked_add(page_size).unwrap()),
                        )?;
                        virt = virt.checked_add(page_size).unwrap();
                        phys = phys.checked_add(page_size).unwrap();
                        remaining_bytes -= page_size;
                        continue 'outer;
                    } else if !pte.is_valid() {
                        // The current PTE is vacant, but we couldn't map at this level (because the
                        // page size was too large, or the request wasn't sufficiently aligned or
                        // because the architecture just can't map at this level). This means
                        // we need to allocate a new sub-table and retry.
                        // allocate a new physical frame to hold the next level table and
                        // mark this PTE as a valid internal node pointing to that sub-table.
                        let frame = frame_alloc::alloc_one_zeroed()?;

                        // TODO memory barrier

                        pte.replace_address_and_flags(frame.addr(), PTEFlags::VALID);
                        pgtable = self.pgtable_ptr_from_phys(frame.addr());
                        self.wired_frames.push(frame);
                    }
                } else if !pte.is_leaf() {
                    // This PTE is an internal node pointing to another page table
                    pgtable = self.pgtable_ptr_from_phys(pte.get_address_and_flags().0);
                } else {
                    unreachable!("Invalid state: PTE can't be valid leaf (this means {virt:?} is already mapped) {pte:?} {pte:p}");
                }
            }
        }

        Ok(())
    }

    unsafe fn remap_contiguous(
        &mut self,
        mut virt: VirtualAddress,
        mut phys: PhysicalAddress,
        len: NonZeroUsize,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        let mut remaining_bytes = len.get();
        debug_assert!(
            remaining_bytes >= PAGE_SIZE,
            "virtual address range must span be at least one page"
        );
        debug_assert!(
            virt.is_aligned_to(PAGE_SIZE),
            "virtual address must be aligned to at least 4KiB page size"
        );
        debug_assert!(
            phys.is_aligned_to(PAGE_SIZE),
            "physical address must be aligned to at least 4KiB page size"
        );

        // This algorithm behaves a lot like the above one for `map_contiguous` but with an important
        // distinction: Since we require the entire range to already be mapped, we follow the page tables
        // until we reach a valid PTE. Once we reached, we assert that we can map the given physical
        // address here and simply update the PTEs address. We then again repeat this until we have
        // no more bytes to process.
        'outer: while remaining_bytes > 0 {
            let mut pgtable = self.pgtable_ptr_from_phys(self.root_pgtable);

            for lvl in (0..PAGE_TABLE_LEVELS).rev() {
                let pte = unsafe {
                    let index = pte_index_for_level(virt, lvl);
                    pgtable.add(index).as_mut()
                };

                if pte.is_valid() && pte.is_leaf() {
                    // We reached the previously mapped leaf node that we want to edit
                    // assert that we can actually map at this level (remap requires users to remap
                    // only to equal or larger alignments, but we should make sure.
                    let page_size = page_size_for_level(lvl);

                    debug_assert!(
                        can_map_at_level(virt, phys, remaining_bytes, lvl),
                        "remapping requires the same alignment ({page_size}) but found {phys:?}, {remaining_bytes}bytes"
                    );

                    let (_old_phys, flags) = pte.get_address_and_flags();
                    pte.replace_address_and_flags(phys, flags);

                    flush.extend_range(
                        self.asid,
                        Range::from(virt..virt.checked_add(page_size).unwrap()),
                    )?;
                    virt = virt.checked_add(page_size).unwrap();
                    phys = phys.checked_add(page_size).unwrap();
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

    unsafe fn protect(
        &mut self,
        mut virt: VirtualAddress,
        len: NonZeroUsize,
        new_flags: Self::Flags,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        let mut remaining_bytes = len.get();
        debug_assert!(
            remaining_bytes >= PAGE_SIZE,
            "virtual address range must span be at least one page"
        );
        debug_assert!(
            virt.is_aligned_to(PAGE_SIZE),
            "virtual address must be aligned to at least 4KiB page size"
        );

        // The algorithm below is essentially the same as `remap_contiguous` but with the difference
        // that we don't replace the PTEs address but instead the PTEs flags ensuring that the caller
        // can't increase the permissions.
        'outer: while remaining_bytes > 0 {
            let mut pgtable = self.pgtable_ptr_from_phys(self.root_pgtable);

            for lvl in (0..PAGE_TABLE_LEVELS).rev() {
                let pte = unsafe {
                    let index = pte_index_for_level(virt, lvl);
                    pgtable.add(index).as_mut()
                };

                if pte.is_valid() && pte.is_leaf() {
                    // We reached the previously mapped leaf node that we want to edit
                    // firstly, ensure that this operation only removes permissions never adds any
                    // and secondly mask out the old permissions replacing them with the new. This must
                    // ensure we retain any other flags in the process.
                    let rwx_mask = PTEFlags::READ | PTEFlags::WRITE | PTEFlags::EXECUTE;

                    let new_flags = rwx_mask & new_flags;
                    let (phys, old_flags) = pte.get_address_and_flags();

                    // ensure!(
                    //     old_flags.intersection(rwx_mask).contains(new_flags),
                    //     Error::PermissionIncrease
                    // );

                    pte.replace_address_and_flags(
                        phys,
                        old_flags.difference(rwx_mask).union(new_flags),
                    );

                    let page_size = page_size_for_level(lvl);
                    flush.extend_range(
                        self.asid,
                        Range::from(virt..virt.checked_add(page_size).unwrap()),
                    )?;
                    virt = virt.checked_add(page_size).unwrap();
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

    unsafe fn unmap(
        &mut self,
        mut virt: VirtualAddress,
        len: NonZeroUsize,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        let mut remaining_bytes = len.get();
        debug_assert!(
            remaining_bytes >= PAGE_SIZE,
            "virtual address range must span be at least one page"
        );
        debug_assert!(
            virt.is_aligned_to(PAGE_SIZE),
            "virtual address must be aligned to at least 4KiB page size"
        );

        // The algorithm of this function is different from the others, it processes the requested range
        // in an iterative fashion, but will not traverse the page table levels iteratively instead
        // doing so recursively. The reason for this is that after unmapping a page we need to check
        // if by doing so the parent has become empty in which case we also need to unmap the parent.
        while remaining_bytes > 0 {
            unsafe {
                self.unmap_inner(
                    self.pgtable_ptr_from_phys(self.root_pgtable),
                    &mut virt,
                    &mut remaining_bytes,
                    PAGE_TABLE_LEVELS - 1,
                    flush,
                )?;
            }
        }

        Ok(())
    }

    unsafe fn query(&mut self, virt: VirtualAddress) -> Option<(PhysicalAddress, Self::Flags)> {
        let mut pgtable: NonNull<PageTableEntry> = self.pgtable_ptr_from_phys(self.root_pgtable);

        for lvl in (0..PAGE_TABLE_LEVELS).rev() {
            let pte = unsafe {
                let index = pte_index_for_level(virt, lvl);
                pgtable.add(index).as_mut()
            };

            if pte.is_valid() && pte.is_leaf() {
                let (addr, flags) = pte.get_address_and_flags();
                return Some((addr, flags));
            } else if pte.is_valid() {
                // This PTE is an internal node pointing to another page table
                pgtable = self.pgtable_ptr_from_phys(pte.get_address_and_flags().0);
            } else {
                // This PTE is vacant, which means at whatever level we're at, there is no
                // point at doing any more work since this address cannot be mapped to anything
                // anyway.

                return None;
            }
        }

        None
    }

    unsafe fn activate(&self) {
        unsafe {
            let ppn = self.root_pgtable.get() >> 12;
            satp::set(satp::Mode::Sv39, DEFAULT_ASID, ppn);
        }
    }

    fn new_flush(&self) -> Flush {
        todo!()
    }
}

impl AddressSpace {
    unsafe fn unmap_inner(
        &mut self,
        pgtable: NonNull<PageTableEntry>,
        virt: &mut VirtualAddress,
        remaining_bytes: &mut usize,
        lvl: usize,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        let index = pte_index_for_level(*virt, lvl);
        let pte = unsafe { pgtable.add(index).as_mut() };
        let page_size = page_size_for_level(lvl);

        if pte.is_valid() && pte.is_leaf() {
            // The PTE is mapped, so go ahead and clear it unmapping the frame
            pte.clear();

            flush.extend_range(
                self.asid,
                Range::from(*virt..virt.checked_add(page_size).unwrap()),
            )?;
            *virt = virt.checked_add(page_size).unwrap();
            *remaining_bytes -= page_size;
        } else if pte.is_valid() {
            // This PTE is an internal node pointing to another page table
            let pgtable = self.pgtable_ptr_from_phys(pte.get_address_and_flags().0);
            unsafe {
                self.unmap_inner(pgtable, virt, remaining_bytes, lvl - 1, flush)?;
            }

            // The recursive descend above might have unmapped the last child of this PTE in which
            // case we need to unmap it as well

            // TODO optimize this
            let is_still_populated = (0..PAGE_TABLE_ENTRIES)
                .any(|index| unsafe { pgtable.add(index).as_ref().is_valid() });

            if !is_still_populated {
                let frame = pte.get_address_and_flags().0;
                self.wired_frames.retain(|wired| wired.addr() != frame);
                pte.clear();
            }
        } else {
            unreachable!("Invalid state: PTE cant be invalid (this means {virt:?} is already unmapped) {pte:?}");
        }

        Ok(())
    }

    fn pgtable_ptr_from_phys(&self, phys: PhysicalAddress) -> NonNull<PageTableEntry> {
        NonNull::new(
            KERNEL_ASPACE_BASE
                .checked_add(phys.get())
                .unwrap()
                .as_mut_ptr()
                .cast(),
        )
        .unwrap()
    }
}

#[repr(transparent)]
pub struct PageTableEntry {
    bits: usize,
}

impl fmt::Debug for PageTableEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rsw = (self.bits & ((1 << 2) - 1) << 8) >> 8;
        let ppn0 = (self.bits & ((1 << 9) - 1) << 10) >> 10;
        let ppn1 = (self.bits & ((1 << 9) - 1) << 19) >> 19;
        let ppn2 = (self.bits & ((1 << 26) - 1) << 28) >> 28;
        let reserved = (self.bits & ((1 << 7) - 1) << 54) >> 54;
        let pbmt = (self.bits & ((1 << 2) - 1) << 61) >> 61;
        let n = (self.bits & ((1 << 1) - 1) << 63) >> 63;

        f.debug_struct("PageTableEntry")
            .field("n", &format_args!("{n:01b}"))
            .field("pbmt", &format_args!("{pbmt:02b}"))
            .field("reserved", &format_args!("{reserved:07b}"))
            .field("ppn2", &format_args!("{ppn2:026b}"))
            .field("ppn1", &format_args!("{ppn1:09b}"))
            .field("ppn0", &format_args!("{ppn0:09b}"))
            .field("rsw", &format_args!("{rsw:02b}"))
            .field("flags", &self.get_address_and_flags().1)
            .finish()
    }
}

impl PageTableEntry {
    pub fn is_valid(&self) -> bool {
        PTEFlags::from_bits_retain(self.bits).contains(PTEFlags::VALID)
    }

    pub fn is_leaf(&self) -> bool {
        PTEFlags::from_bits_retain(self.bits)
            .intersects(PTEFlags::READ | PTEFlags::WRITE | PTEFlags::EXECUTE)
    }

    pub fn replace_address_and_flags(&mut self, address: PhysicalAddress, flags: PTEFlags) {
        self.bits &= PTEFlags::all().bits(); // clear all previous flags
        self.bits |= (address.get() >> PTE_PPN_SHIFT) | flags.bits();
    }

    pub fn get_address_and_flags(&self) -> (PhysicalAddress, PTEFlags) {
        // TODO correctly mask out address
        let addr = PhysicalAddress::new((self.bits & !PTEFlags::all().bits()) << PTE_PPN_SHIFT);
        let flags = PTEFlags::from_bits_truncate(self.bits);
        (addr, flags)
    }

    pub fn clear(&mut self) {
        self.bits = 0;
    }
}

bitflags! {
    #[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
    pub struct PTEFlags: usize {
        const VALID     = 1 << 0;
        const READ      = 1 << 1;
        const WRITE     = 1 << 2;
        const EXECUTE   = 1 << 3;
        const USER      = 1 << 4;
        const GLOBAL    = 1 << 5;
        const ACCESSED    = 1 << 6;
        const DIRTY     = 1 << 7;
    }
}

impl From<crate::vm::Permissions> for PTEFlags {
    fn from(flags: crate::vm::Permissions) -> Self {
        use crate::vm::Permissions;

        let mut out = Self::VALID | Self::DIRTY | Self::ACCESSED;

        for flag in flags {
            match flag {
                Permissions::READ => out.insert(Self::READ),
                Permissions::WRITE => out.insert(Self::WRITE),
                Permissions::EXECUTE => out.insert(Self::EXECUTE),
                _ => unreachable!(),
            }
        }

        out
    }
}
