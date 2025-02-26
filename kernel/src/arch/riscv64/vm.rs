// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch::{mb, wmb};
use crate::vm::Error;
use crate::vm::flush::Flush;
use crate::vm::frame_alloc::Frame;
use crate::vm::{PhysicalAddress, VirtualAddress, frame_alloc};
use alloc::vec;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::num::NonZeroUsize;
use core::ptr::NonNull;
use core::range::{Range, RangeInclusive};
use core::{fmt, slice};
use riscv::satp;
use riscv::sbi::rfence::sfence_vma_asid;
use static_assertions::const_assert_eq;

pub const DEFAULT_ASID: u16 = 0;

pub const KERNEL_ASPACE_RANGE: RangeInclusive<VirtualAddress> = RangeInclusive {
    start: VirtualAddress::new(0xffffffc000000000).unwrap(),
    end: VirtualAddress::MAX,
};
const_assert_eq!(KERNEL_ASPACE_RANGE.start.get(), CANONICAL_ADDRESS_MASK);
const_assert_eq!(
    KERNEL_ASPACE_RANGE
        .end
        .checked_sub_addr(KERNEL_ASPACE_RANGE.start)
        .unwrap(),
    !CANONICAL_ADDRESS_MASK
);

/// Virtual address where the user address space starts.
///
/// The first 2MiB are reserved for catching null pointer dereferences, but this might
/// change in the future if we decide that the null-checking performed by the WASM runtime
/// is sufficiently robust.
pub const USER_ASPACE_RANGE: RangeInclusive<VirtualAddress> = RangeInclusive {
    start: VirtualAddress::new(0x0000000000200000).unwrap(),
    end: VirtualAddress::new((1 << VIRT_ADDR_BITS) - 1).unwrap(),
};

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

#[cold]
pub fn init() {
    let root_pgtable = get_active_pgtable(DEFAULT_ASID);

    // Zero out the lower half of the kernel address space to remove e.g. the leftover loader identity mappings
    // Safety: `get_active_pgtable` & `VirtualAddress::from_phys` do minimal checking that the address is valid
    // but otherwise we have to trust the address is valid for the entire page.
    unsafe {
        slice::from_raw_parts_mut(
            VirtualAddress::from_phys(root_pgtable)
                .unwrap()
                .as_mut_ptr(),
            PAGE_SIZE / 2,
        )
        .fill(0);
    }

    wmb();
}

/// Return whether the given virtual address is in the kernel address space.
pub const fn is_kernel_address(virt: VirtualAddress) -> bool {
    KERNEL_ASPACE_RANGE.start.get() <= virt.get() && virt.get() < KERNEL_ASPACE_RANGE.end.get()
}

/// Invalidate address translation caches for the given `address_range` in the given `address_space`.
///
/// # Errors
///
/// Should return an error if the underlying operation failed and the caches could not be invalidated.
pub fn invalidate_range(asid: u16, address_range: Range<VirtualAddress>) -> Result<(), Error> {
    mb();

    let base_addr = address_range.start.get();
    let size = address_range
        .end
        .checked_sub_addr(address_range.start)
        .unwrap();
    sfence_vma_asid(0, usize::MAX, base_addr, size, asid)?;

    mb();

    Ok(())
}

/// Return the page size for the given page table level.
///
/// # Panics
///
/// Panics if the provided level is `>= PAGE_TABLE_LEVELS`.
fn page_size_for_level(level: usize) -> usize {
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
fn pte_index_for_level(virt: VirtualAddress, lvl: usize) -> usize {
    assert!(lvl < PAGE_TABLE_LEVELS);
    let index = (virt.get() >> (PAGE_SHIFT + lvl * PAGE_ENTRY_SHIFT)) & (PAGE_TABLE_ENTRIES - 1);
    debug_assert!(index < PAGE_TABLE_ENTRIES);

    index
}

/// Return whether the combination of `virt`,`phys`, and `remaining_bytes` can be mapped at the given `level`.
///
/// This is the case when both the virtual and physical address are aligned to the page size at this level
/// AND the remaining size is at least the page size.
fn can_map_at_level(
    virt: VirtualAddress,
    phys: PhysicalAddress,
    remaining_bytes: usize,
    lvl: usize,
) -> bool {
    let page_size = page_size_for_level(lvl);
    virt.is_aligned_to(page_size) && phys.is_aligned_to(page_size) && remaining_bytes >= page_size
}

fn get_active_pgtable(expected_asid: u16) -> PhysicalAddress {
    let satp = satp::read();
    assert_eq!(satp.asid(), expected_asid);
    let root_pgtable = PhysicalAddress::new(satp.ppn() << 12);
    debug_assert!(root_pgtable.get() != 0);
    root_pgtable
}

pub struct AddressSpace {
    root_pgtable: PhysicalAddress,
    wired_frames: Vec<Frame>,
    asid: u16,
}

impl crate::vm::ArchAddressSpace for AddressSpace {
    type Flags = PTEFlags;

    fn new(asid: u16) -> Result<(Self, Flush), Error>
    where
        Self: Sized,
    {
        // Safety: we just allocated the page and we're only accessing the upper half of it
        let src = unsafe {
            let satp = satp::read();
            let root_pgtable = PhysicalAddress::new(satp.ppn() << 12);
            debug_assert!(root_pgtable.get() != 0);

            let base = VirtualAddress::from_phys(root_pgtable)
                .unwrap()
                .checked_add(PAGE_SIZE / 2)
                .unwrap()
                .as_ptr();

            slice::from_raw_parts(base, PAGE_SIZE / 2)
        };

        let mut root_pgtable = frame_alloc::alloc_one_zeroed()?;

        Frame::get_mut(&mut root_pgtable).unwrap().as_mut_slice()[PAGE_SIZE / 2..]
            .copy_from_slice(src);

        mb();

        let this = Self {
            asid,
            root_pgtable: root_pgtable.addr(),
            wired_frames: vec![root_pgtable],
        };

        Ok((this, Flush::empty(asid)))
    }

    fn from_active(asid: u16) -> (Self, Flush)
    where
        Self: Sized,
    {
        let root_pgtable = get_active_pgtable(asid);

        let this = Self {
            asid,
            root_pgtable,
            wired_frames: vec![],
        };

        (this, Flush::empty(asid))
    }

    unsafe fn map_contiguous(
        &mut self,
        mut virt: VirtualAddress,
        mut phys: PhysicalAddress,
        len: NonZeroUsize,
        flags: Self::Flags,
        flush: &mut Flush,
    ) -> Result<(), Error> {
        let mut remaining_bytes = len.get();
        if flags.contains(PTEFlags::WRITE) {
            debug_assert!(
                flags.contains(PTEFlags::READ),
                "writable pages must also be marked readable"
            );
        }
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

                // Safety: index is always within one page
                let pte = unsafe { pgtable.add(index).as_mut() };

                // Let's check if we can map at this level of the page table given our
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
                } else if pte.is_valid() && !pte.is_leaf() {
                    // This PTE is an internal node pointing to another page table
                    pgtable = self.pgtable_ptr_from_phys(pte.get_address_and_flags().0);
                } else {
                    // The current PTE is vacant, but we couldn't map at this level (because the
                    // page size was too large, or the request wasn't sufficiently aligned or
                    // because the architecture just can't map at this level). This means
                    // we need to allocate a new sub-table and retry.
                    // allocate a new physical frame to hold the next level table and
                    // mark this PTE as a valid internal node pointing to that sub-table.
                    let frame = frame_alloc::alloc_one_zeroed()?;

                    mb();

                    pte.replace_address_and_flags(frame.addr(), PTEFlags::VALID);
                    pgtable = self.pgtable_ptr_from_phys(frame.addr());
                    self.wired_frames.push(frame);
                }
            }
        }

        mb();

        Ok(())
    }

    unsafe fn update_flags(
        &mut self,
        mut virt: VirtualAddress,
        len: NonZeroUsize,
        new_flags: Self::Flags,
        flush: &mut Flush,
    ) -> Result<(), Error> {
        let mut remaining_bytes = len.get();
        if new_flags.contains(PTEFlags::WRITE) {
            debug_assert!(
                new_flags.contains(PTEFlags::READ),
                "writable pages must also be marked readable"
            );
        }
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
                // Safety: index is always within one page
                let pte = unsafe {
                    let index = pte_index_for_level(virt, lvl);
                    pgtable.add(index).as_mut()
                };
                let page_size = page_size_for_level(lvl);

                if pte.is_valid() && pte.is_leaf() {
                    // We reached the previously mapped leaf node that we want to edit
                    // firstly, ensure that this operation only removes permissions never adds any
                    // and secondly mask out the old permissions replacing them with the new. This must
                    // ensure we retain any other flags in the process.
                    let rwx_mask = PTEFlags::READ | PTEFlags::WRITE | PTEFlags::EXECUTE;

                    let new_flags = rwx_mask & new_flags;
                    let (phys, old_flags) = pte.get_address_and_flags();

                    pte.replace_address_and_flags(
                        phys,
                        old_flags.difference(rwx_mask).union(new_flags),
                    );

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
                    remaining_bytes = remaining_bytes.saturating_sub(page_size);
                }
            }
        }

        mb();

        Ok(())
    }

    unsafe fn unmap(
        &mut self,
        mut virt: VirtualAddress,
        len: NonZeroUsize,
        flush: &mut Flush,
    ) -> Result<(), Error> {
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
            self.unmap_inner(
                self.pgtable_ptr_from_phys(self.root_pgtable),
                &mut virt,
                &mut remaining_bytes,
                PAGE_TABLE_LEVELS - 1,
                flush,
            )?;
        }

        mb();

        Ok(())
    }

    unsafe fn query(&mut self, virt: VirtualAddress) -> Option<(PhysicalAddress, Self::Flags)> {
        let mut pgtable: NonNull<PageTableEntry> = self.pgtable_ptr_from_phys(self.root_pgtable);

        for lvl in (0..PAGE_TABLE_LEVELS).rev() {
            // Safety: index is always within one page
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
        // Safety: register access
        unsafe {
            let ppn = self.root_pgtable.get() >> 12_i32;
            satp::set(satp::Mode::Sv39, self.asid, ppn);
        }
        mb();
    }

    fn new_flush(&self) -> Flush {
        Flush::empty(self.asid)
    }
}

impl AddressSpace {
    fn unmap_inner(
        &mut self,
        pgtable: NonNull<PageTableEntry>,
        virt: &mut VirtualAddress,
        remaining_bytes: &mut usize,
        lvl: usize,
        flush: &mut Flush,
    ) -> Result<(), Error> {
        let index = pte_index_for_level(*virt, lvl);
        // Safety: index is always within one page
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
            self.unmap_inner(pgtable, virt, remaining_bytes, lvl - 1, flush)?;

            // The recursive descend above might have unmapped the last child of this PTE in which
            // case we need to unmap it as well

            // TODO optimize this
            let is_still_populated = (0..PAGE_TABLE_ENTRIES)
                // Safety: index is always within one page
                .any(|index| unsafe { pgtable.add(index).as_ref().is_valid() });

            if !is_still_populated {
                let frame = pte.get_address_and_flags().0;
                self.wired_frames.retain(|wired| wired.addr() != frame);
                pte.clear();
            }
        } else {
            *remaining_bytes = remaining_bytes.saturating_sub(page_size);
        }

        Ok(())
    }

    fn pgtable_ptr_from_phys(&self, phys: PhysicalAddress) -> NonNull<PageTableEntry> {
        NonNull::new(
            KERNEL_ASPACE_RANGE
                .start
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
        let rsw = (self.bits & ((1 << 2_i32) - 1) << 8_i32) >> 8_i32;
        let ppn0 = (self.bits & ((1 << 9_i32) - 1) << 10_i32) >> 10_i32;
        let ppn1 = (self.bits & ((1 << 9_i32) - 1) << 19_i32) >> 19_i32;
        let ppn2 = (self.bits & ((1 << 26_i32) - 1) << 28_i32) >> 28_i32;
        let reserved = (self.bits & ((1 << 7_i32) - 1) << 54_i32) >> 54_i32;
        let pbmt = (self.bits & ((1 << 2_i32) - 1) << 61_i32) >> 61_i32;
        let n = (self.bits & ((1 << 1_i32) - 1) << 63_i32) >> 63_i32;

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
        self.bits = (address.get() >> PTE_PPN_SHIFT) | flags.bits();
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
        /// Indicates the page table entry is initialized
        const VALID     = 1 << 0;
        /// Whether the page is readable
        const READ      = 1 << 1;
        /// Whether the page is writable
        const WRITE     = 1 << 2;
        /// Whether the page is executable
        const EXECUTE   = 1 << 3;
        /// Whether the page is accessible to user mode.
        ///
        /// By default, pages are only accessible in supervisor mode but marking a page as user-accessible
        /// allows user mode code to access the page too.
        const USER      = 1 << 4;
        /// Designates a global mapping.
        ///
        /// Global mappings exist in all address space.
        ///
        /// Note that as stated in the RISCV privileged spec, forgetting to mark a global mapping as global
        /// is *fine* since it just results in slower performance. However, marking a non-global mapping as
        /// global by accident will result in undefined behaviour (the CPU might use any of the competing
        /// mappings for the address).
        const GLOBAL    = 1 << 5;
        /// Indicated the page has been read, written, or executed from.
        const ACCESSED    = 1 << 6;
        /// Indicates the page has been written to.
        const DIRTY     = 1 << 7;
    }
}

impl From<crate::vm::Permissions> for PTEFlags {
    fn from(flags: crate::vm::Permissions) -> Self {
        use crate::vm::Permissions;

        // we currently don't use the accessed & dirty bits and, it's recommended to set them if unused
        let mut out = Self::VALID | Self::ACCESSED | Self::DIRTY;

        for flag in flags {
            match flag {
                Permissions::READ => out.insert(Self::READ),
                Permissions::WRITE => out.insert(Self::WRITE),
                Permissions::EXECUTE => out.insert(Self::EXECUTE),
                Permissions::USER => out.insert(Self::USER),
                _ => unreachable!(),
            }
        }

        out
    }
}
