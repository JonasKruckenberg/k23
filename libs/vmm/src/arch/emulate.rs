use crate::entry::Entry;
use crate::{Mode, PhysicalAddress, VirtualAddress};
use bitflags::bitflags;
use core::ops::Range;

bitflags! {
    #[derive(Debug, Copy, Clone)]
    pub struct EmulateEntryFlags: usize {
        const VALID = 1 << 0;
        const READ = 1 << 1;
        const WRITE = 1 << 2;
        const EXECUTE = 1 << 3;
        const USER = 1 << 4;
    }
}

impl From<usize> for EmulateEntryFlags {
    fn from(value: usize) -> Self {
        Self::from_bits_truncate(value)
    }
}

impl Into<usize> for EmulateEntryFlags {
    fn into(self) -> usize {
        self.bits()
    }
}

macro_rules! get_bits {
    ($num: expr, length: $length: expr, offset: $offset: expr) => {
        ($num & (((1 << $length) - 1) << $offset)) >> $offset
    };
}

/// Mock RiscvSv39 architecture for testing
pub struct EmulateArch;

impl EmulateArch {
    pub fn virt_from_parts(
        vpn2: usize,
        vpn1: usize,
        vpn0: usize,
        page_offset: usize,
    ) -> VirtualAddress {
        let raw = ((vpn2 << 30) | (vpn1 << 21) | (vpn0 << 12) | page_offset) as isize;
        let shift = 64 * 8 - 38;
        VirtualAddress(raw.wrapping_shl(shift).wrapping_shr(shift) as usize)
    }

    pub fn virt_into_parts(virt: VirtualAddress) -> (usize, usize, usize, usize) {
        let vpn2 = get_bits!(virt.0, length: 9, offset: 30);
        let vpn1 = get_bits!(virt.0, length: 9, offset: 21);
        let vpn0 = get_bits!(virt.0, length: 9, offset: 12);
        let offset = virt.0 & Self::PAGE_OFFSET_MASK;
        (vpn2, vpn1, vpn0, offset)
    }
}

impl Mode for EmulateArch {
    type EntryFlags = EmulateEntryFlags;

    const PHYS_OFFSET: usize = 0xffff_ffd8_0000_0000;

    const PAGE_SIZE: usize = 4096;

    const PAGE_TABLE_LEVELS: usize = 2; // L0, L1, L2
    const PAGE_TABLE_ENTRIES: usize = 512;

    const ENTRY_FLAG_DEFAULT_LEAF: Self::EntryFlags = EmulateEntryFlags::VALID;
    const ENTRY_FLAG_DEFAULT_TABLE: Self::EntryFlags = EmulateEntryFlags::VALID;

    fn invalidate_all() -> crate::Result<()> {
        Ok(())
    }

    fn invalidate_range(_asid: usize, _address_range: Range<VirtualAddress>) -> crate::Result<()> {
        Ok(())
    }

    fn get_active_table(_asid: usize) -> PhysicalAddress {
        PhysicalAddress(0)
    }

    fn activate_table(_asid: usize, _table: VirtualAddress) {}

    fn entry_is_leaf(entry: &Entry<Self>) -> bool
    where
        Self: Sized,
    {
        // A table entry is a leaf if it has the read and execute flags set
        entry
            .get_flags()
            .intersects(EmulateEntryFlags::READ | EmulateEntryFlags::EXECUTE)
    }

    fn phys_to_virt(phys: PhysicalAddress) -> VirtualAddress {
        unsafe { VirtualAddress::new(phys.as_raw()) }
    }
}
