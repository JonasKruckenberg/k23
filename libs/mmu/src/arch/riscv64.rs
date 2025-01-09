// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch::PAGE_TABLE_ENTRIES;
use crate::{PhysicalAddress, VirtualAddress};
use bitflags::bitflags;
use core::fmt;
use core::range::Range;
use riscv::satp;
use riscv::sbi::rfence::sfence_vma_asid;
use static_assertions::const_assert_eq;

/// Number of bits we need to shift an address by to reach the next page
pub const PAGE_SHIFT: usize = 12; // 4096 bytes

pub const PAGE_TABLE_LEVELS: usize = 3; // L0, L1, L2 Sv39

pub const PAGE_ENTRY_SHIFT: usize = 9; // 512 entries, 8 bytes each

pub const VIRT_ADDR_BITS: u32 = 38;

/// Canonical addresses are addresses where the tops bits (`VIRT_ADDR_BITS` to 63)
/// are all either 0 or 1.
pub const CANONICAL_ADDRESS_MASK: usize = !((1 << (VIRT_ADDR_BITS)) - 1);
const_assert_eq!(CANONICAL_ADDRESS_MASK, 0xffffffc000000000);

pub const PTE_FLAGS_VALID: PTEFlags = PTEFlags::VALID;
pub const PTE_FLAGS_RWX_MASK: PTEFlags = PTEFlags::from_bits_retain(
    PTEFlags::READ.bits() | PTEFlags::WRITE.bits() | PTEFlags::EXECUTE.bits(),
);

/// On `RiscV` targets the page table entry's physical address bits are shifted 2 bits to the right.
const PTE_PPN_SHIFT: usize = 2;

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
    let base_addr = address_range.start.0;
    let size = address_range.end.0 - address_range.start.0;
    sfence_vma_asid(0, usize::MAX, base_addr, size, asid)?;
    Ok(())
}

/// Return a pointer to the currently active page table.
pub fn get_active_pgtable(asid: usize) -> PhysicalAddress {
    let satp = satp::read();
    assert_eq!(satp.asid(), asid);
    PhysicalAddress(satp.ppn() << 12)
}

/// Set the given page table as the currently active page table.
///
/// # Safety
///
/// This will invalidate pointers if not used carefully
pub unsafe fn activate_pgtable(asid: usize, pgtable: PhysicalAddress) {
    unsafe {
        let ppn = pgtable.get() >> 12;
        satp::set(satp::Mode::Sv39, asid, ppn);
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
        self.bits |= (address.0 >> PTE_PPN_SHIFT) | flags.bits();
    }

    pub fn get_address_and_flags(&self) -> (PhysicalAddress, PTEFlags) {
        // TODO correctly mask out address
        let addr = PhysicalAddress((self.bits & !PTEFlags::all().bits()) << PTE_PPN_SHIFT);
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

impl From<crate::Flags> for PTEFlags {
    fn from(flags: crate::Flags) -> Self {
        use crate::Flags;

        let mut out = Self::VALID | Self::DIRTY | Self::ACCESSED;

        for flag in flags {
            match flag {
                Flags::READ => out.insert(Self::READ),
                Flags::WRITE => out.insert(Self::WRITE),
                Flags::EXECUTE => out.insert(Self::EXECUTE),
                _ => unreachable!(),
            }
        }

        out
    }
}

impl From<PTEFlags> for crate::Flags {
    fn from(arch_flags: PTEFlags) -> Self {
        use crate::Flags;
        let mut out = Flags::empty();

        for flag in arch_flags {
            match flag {
                PTEFlags::READ => out.insert(Self::READ),
                PTEFlags::WRITE => out.insert(Self::WRITE),
                PTEFlags::EXECUTE => out.insert(Self::EXECUTE),
                _ => unreachable!(),
            }
        }

        out
    }
}
