use crate::entry::Entry;
use crate::{Mode, PhysicalAddress, VirtualAddress};
use bitflags::bitflags;
use core::ops::Range;
use riscv::satp;
use riscv::sbi::rfence::{sfence_vma, sfence_vma_asid};

bitflags! {
    #[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
    pub struct EntryFlags: usize {
        const VALID     = 1 << 0;
        const READ      = 1 << 1;
        const WRITE     = 1 << 2;
        const EXECUTE   = 1 << 3;
        const USER      = 1 << 4;
        const GLOBAL    = 1 << 5;
        const ACCESS    = 1 << 6;
        const DIRTY     = 1 << 7;
    }
}

impl From<usize> for EntryFlags {
    fn from(value: usize) -> Self {
        Self::from_bits_truncate(value)
    }
}

impl From<EntryFlags> for usize {
    fn from(value: EntryFlags) -> Self {
        value.bits()
    }
}

const PAGE_SIZE: usize = 4096;
const PAGE_TABLE_ENTRIES: usize = 512;
const ENTRY_ADDRESS_SHIFT: usize = 2;

const ENTRY_FLAGS_LEAF: EntryFlags = EntryFlags::VALID;
const ENTRY_FLAGS_TABLE: EntryFlags = EntryFlags::VALID;
const ENTRY_FLAGS_RX: EntryFlags = EntryFlags::READ.union(EntryFlags::EXECUTE);
const ENTRY_FLAGS_RO: EntryFlags = EntryFlags::READ;
const ENTRY_FLAGS_RW: EntryFlags = EntryFlags::READ.union(EntryFlags::WRITE);

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

#[derive(Debug, Clone, Copy)]
pub struct Riscv64Sv39;

impl Mode for Riscv64Sv39 {
    type EntryFlags = EntryFlags;

    const PAGE_SIZE: usize = PAGE_SIZE;
    const PAGE_TABLE_LEVELS: usize = 3; // L0, L1, L2
    const PAGE_TABLE_ENTRIES: usize = PAGE_TABLE_ENTRIES;

    const ENTRY_FLAGS_LEAF: Self::EntryFlags = ENTRY_FLAGS_LEAF;
    const ENTRY_FLAGS_TABLE: Self::EntryFlags = ENTRY_FLAGS_TABLE;
    const ENTRY_FLAGS_RX: Self::EntryFlags = ENTRY_FLAGS_RX;
    const ENTRY_FLAGS_RO: Self::EntryFlags = ENTRY_FLAGS_RO;
    const ENTRY_FLAGS_RW: Self::EntryFlags = ENTRY_FLAGS_RW;

    const ENTRY_ADDRESS_SHIFT: usize = ENTRY_ADDRESS_SHIFT;

    fn invalidate_all() -> crate::Result<()> {
        sfence_vma(0, usize::MAX, 0, usize::MAX)?;
        Ok(())
    }

    fn invalidate_range(asid: usize, address_range: Range<VirtualAddress>) -> crate::Result<()> {
        invalidate_address_range(asid, address_range)
    }

    fn get_active_table(_asid: usize) -> PhysicalAddress {
        unsafe { get_active_table() }
    }

    fn activate_table(asid: usize, table: VirtualAddress) {
        unsafe {
            let ppn = table.as_raw() >> 12;
            satp::set(satp::Mode::Sv39, asid, ppn);
        }
    }

    fn entry_is_leaf(entry: &Entry<Self>) -> bool
    where
        Self: Sized,
    {
        // A table entry is a leaf if it has the read and execute flags set
        entry
            .get_flags()
            .intersects(EntryFlags::READ | EntryFlags::EXECUTE)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Riscv64Sv48;

impl Mode for Riscv64Sv48 {
    type EntryFlags = EntryFlags;

    const PAGE_SIZE: usize = PAGE_SIZE;
    const PAGE_TABLE_LEVELS: usize = 4; // L0, L1, L2, L3
    const PAGE_TABLE_ENTRIES: usize = PAGE_TABLE_ENTRIES;

    const ENTRY_FLAGS_LEAF: Self::EntryFlags = ENTRY_FLAGS_LEAF;
    const ENTRY_FLAGS_TABLE: Self::EntryFlags = ENTRY_FLAGS_TABLE;
    const ENTRY_FLAGS_RX: Self::EntryFlags = ENTRY_FLAGS_RX;
    const ENTRY_FLAGS_RO: Self::EntryFlags = ENTRY_FLAGS_RO;
    const ENTRY_FLAGS_RW: Self::EntryFlags = ENTRY_FLAGS_RW;

    const ENTRY_ADDRESS_SHIFT: usize = ENTRY_ADDRESS_SHIFT;

    fn invalidate_all() -> crate::Result<()> {
        sfence_vma(0, usize::MAX, 0, 0)?;
        Ok(())
    }

    fn invalidate_range(asid: usize, address_range: Range<VirtualAddress>) -> crate::Result<()> {
        invalidate_address_range(asid, address_range)
    }

    fn get_active_table(_asid: usize) -> PhysicalAddress {
        unsafe { get_active_table() }
    }

    fn activate_table(asid: usize, table: VirtualAddress) {
        unsafe {
            let ppn = table.as_raw() >> 12;
            satp::set(satp::Mode::Sv48, asid, ppn);
        }
    }

    fn entry_is_leaf(entry: &Entry<Self>) -> bool
    where
        Self: Sized,
    {
        // A table entry is a leaf if it has the read and execute flags set
        entry
            .get_flags()
            .intersects(EntryFlags::READ | EntryFlags::EXECUTE)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Riscv64Sv57;

impl Mode for Riscv64Sv57 {
    type EntryFlags = EntryFlags;

    const PAGE_SIZE: usize = PAGE_SIZE;
    const PAGE_TABLE_LEVELS: usize = 5; // L0, L1, L2, L3, L4
    const PAGE_TABLE_ENTRIES: usize = PAGE_TABLE_ENTRIES;

    const ENTRY_FLAGS_LEAF: Self::EntryFlags = ENTRY_FLAGS_LEAF;
    const ENTRY_FLAGS_TABLE: Self::EntryFlags = ENTRY_FLAGS_TABLE;
    const ENTRY_FLAGS_RX: Self::EntryFlags = ENTRY_FLAGS_RX;
    const ENTRY_FLAGS_RO: Self::EntryFlags = ENTRY_FLAGS_RO;
    const ENTRY_FLAGS_RW: Self::EntryFlags = ENTRY_FLAGS_RW;

    const ENTRY_ADDRESS_SHIFT: usize = ENTRY_ADDRESS_SHIFT;

    fn invalidate_all() -> crate::Result<()> {
        sfence_vma(0, usize::MAX, 0, 0)?;
        Ok(())
    }

    fn invalidate_range(asid: usize, address_range: Range<VirtualAddress>) -> crate::Result<()> {
        invalidate_address_range(asid, address_range)
    }

    fn get_active_table(_asid: usize) -> PhysicalAddress {
        unsafe { get_active_table() }
    }

    fn activate_table(asid: usize, table: VirtualAddress) {
        unsafe {
            let ppn = table.0 >> 12;
            satp::set(satp::Mode::Sv57, asid, ppn);
        }
    }

    fn entry_is_leaf(entry: &Entry<Self>) -> bool
    where
        Self: Sized,
    {
        // A table entry is a leaf if it has the read and execute flags set
        entry
            .get_flags()
            .intersects(EntryFlags::READ | EntryFlags::EXECUTE)
    }
}
