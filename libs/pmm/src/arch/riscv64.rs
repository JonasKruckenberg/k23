use super::Arch;
use crate::{PhysicalAddress, VirtualAddress};
use bitflags::bitflags;
use core::fmt;
use core::ops::Range;
use riscv::satp;
use riscv::sbi::rfence::{sfence_vma, sfence_vma_asid};

/// Number of bits we need to shift an address by to reach the next page
pub const PAGE_SHIFT: usize = 12; // 4096 bytes

/// On `RiscV` targets the page table entry's physical address bits are shifted 2 bits to the right.
const PTE_PPN_SHIFT: usize = 2;

pub struct Riscv64Sv39;

impl Arch for Riscv64Sv39 {
    type PageTableEntry = PageTableEntry;
    const VIRT_ADDR_BITS: u32 = 38;
    const PAGE_TABLE_LEVELS: usize = 3; // L0, L1, L2
    const PAGE_ENTRY_SHIFT: usize = 9; // 512 entries, 8 bytes each

    fn can_map_at_level(
        virt: VirtualAddress,
        phys: PhysicalAddress,
        remaining_bytes: usize,
        level: usize,
    ) -> bool {
        let page_size = Self::page_size_for_level(level);
        virt.is_aligned(page_size) && phys.is_aligned(page_size) && remaining_bytes >= page_size
    }

    fn page_size_for_level(level: usize) -> usize {
        let page_size = 1 << (PAGE_SHIFT + level * Self::PAGE_ENTRY_SHIFT);
        debug_assert!(page_size == 4096 || page_size == 2097152 || page_size == 1073741824);
        page_size
    }

    fn pte_index_for_level(virt: VirtualAddress, level: usize) -> usize {
        debug_assert!(level < Self::PAGE_TABLE_LEVELS);
        let index = (virt.as_raw() >> (PAGE_SHIFT + level * Self::PAGE_ENTRY_SHIFT))
            & (Self::PAGE_TABLE_ENTRIES - 1);
        debug_assert!(index < Self::PAGE_TABLE_ENTRIES);

        index
    }

    fn invalidate_all() -> crate::Result<()> {
        sfence_vma(0, usize::MAX, 0, usize::MAX)?;
        Ok(())
    }

    fn invalidate_range(asid: usize, address_range: Range<VirtualAddress>) -> crate::Result<()> {
        invalidate_address_range(asid, address_range)
    }

    fn get_active_pgtable(_asid: usize) -> PhysicalAddress {
        unsafe { get_active_table() }
    }

    unsafe fn activate_pgtable(asid: usize, pgtable: PhysicalAddress) {
        unsafe {
            let ppn = pgtable.as_raw() >> 12;
            satp::set(satp::Mode::Sv39, asid, ppn);
        }
    }
}

#[repr(transparent)]
pub struct PageTableEntry {
    bits: usize,
}

impl fmt::Debug for PageTableEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use super::PageTableEntry;

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

impl super::PageTableEntry for PageTableEntry {
    type Flags = PageTableEntryFlags;
    const FLAGS_VALID: Self::Flags = PageTableEntryFlags::VALID;
    const FLAGS_RWX: Self::Flags = PageTableEntryFlags::from_bits_retain(
        PageTableEntryFlags::READ.bits()
            | PageTableEntryFlags::WRITE.bits()
            | PageTableEntryFlags::EXECUTE.bits(),
    );

    fn is_valid(&self) -> bool {
        PageTableEntryFlags::from_bits_retain(self.bits).contains(Self::FLAGS_VALID)
    }

    fn is_leaf(&self) -> bool {
        PageTableEntryFlags::from_bits_retain(self.bits).intersects(Self::FLAGS_RWX)
    }

    fn replace_address_and_flags(&mut self, address: PhysicalAddress, flags: Self::Flags) {
        self.bits &= PageTableEntryFlags::all().bits(); // clear all previous flags
        self.bits |= (address.0 >> PTE_PPN_SHIFT) | flags.bits();
    }

    fn get_address_and_flags(&self) -> (PhysicalAddress, Self::Flags) {
        // TODO correctly mask out address
        let addr =
            PhysicalAddress((self.bits & !PageTableEntryFlags::all().bits()) << PTE_PPN_SHIFT);
        let flags = PageTableEntryFlags::from_bits_truncate(self.bits);
        (addr, flags)
    }

    fn clear(&mut self) {
        self.bits = 0;
    }
}

bitflags! {
    #[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
    pub struct PageTableEntryFlags: usize {
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

impl From<crate::Flags> for PageTableEntryFlags {
    fn from(arch_flags: crate::Flags) -> Self {
        use crate::Flags;

        let mut out = Self::VALID | Self::DIRTY | Self::ACCESSED;

        for flag in arch_flags {
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

fn invalidate_address_range(
    asid: usize,
    address_range: Range<VirtualAddress>,
) -> crate::Result<()> {
    let base_addr = address_range.start.0;
    let size = address_range.end.0 - address_range.start.0;
    sfence_vma_asid(0, usize::MAX, base_addr, size, asid)?;
    Ok(())
}

unsafe fn get_active_table() -> PhysicalAddress {
    let satp = satp::read();
    PhysicalAddress(satp.ppn() << 12)
}
