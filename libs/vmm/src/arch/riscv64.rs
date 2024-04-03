use crate::entry::Entry;
use crate::{Mode, PhysicalAddress, VirtualAddress};
use bitflags::bitflags;
use core::ops::Range;
use riscv::register::satp;

bitflags! {
    #[derive(Debug, Copy, Clone)]
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

impl Into<usize> for EntryFlags {
    fn into(self) -> usize {
        self.bits()
    }
}

const PHYS_OFFSET: VirtualAddress = unsafe { VirtualAddress::new(0xffff_ffff_0000_0000) };
// const MMIO_BASE: VirtualAddress = unsafe { VirtualAddress::new(0xffff_ffd8_0000_0000) };

const PAGE_SIZE: usize = 4096;
const PAGE_TABLE_ENTRIES: usize = 512;
const ENTRY_ADDRESS_SHIFT: usize = 2;

const ENTRY_FLAG_DEFAULT_LEAF: EntryFlags = EntryFlags::VALID;
const ENTRY_FLAG_DEFAULT_TABLE: EntryFlags = EntryFlags::VALID;
const ENTRY_FLAG_DEFAULT_READ_WRITE: EntryFlags = EntryFlags::from_bits_truncate(1 << 1 | 1 << 2);

fn invalidate_address_range(
    asid: usize,
    address_range: Range<VirtualAddress>,
) -> crate::Result<()> {
    let base_addr = address_range.start.0;
    let size = address_range.end.0 - address_range.start.0;
    sbicall::rfence::sfence_vma_asid(0, usize::MAX, base_addr, size, asid)?;
    Ok(())
}

unsafe fn get_active_table(asid: usize) -> PhysicalAddress {
    let satp = satp::read();
    assert_eq!(satp.asid(), asid);
    PhysicalAddress(satp.ppn() << 12)
}

#[derive(Debug, Clone, Copy)]
pub struct INIT<M>(M);

impl<M> INIT<M> {
    pub fn into_inner(self) -> M {
        self.0
    }
}

impl<M: Mode> Mode for INIT<M> {
    type EntryFlags = M::EntryFlags;

    const PAGE_SIZE: usize = M::PAGE_SIZE;
    const PAGE_TABLE_LEVELS: usize = M::PAGE_TABLE_LEVELS;
    const PAGE_TABLE_ENTRIES: usize = M::PAGE_TABLE_ENTRIES;

    const ENTRY_FLAG_DEFAULT_LEAF: Self::EntryFlags = M::ENTRY_FLAG_DEFAULT_LEAF;
    const ENTRY_FLAG_DEFAULT_TABLE: Self::EntryFlags = M::ENTRY_FLAG_DEFAULT_TABLE;
    const ENTRY_FLAG_DEFAULT_READ_WRITE: Self::EntryFlags = M::ENTRY_FLAG_DEFAULT_READ_WRITE;

    const ENTRY_ADDRESS_SHIFT: usize = M::ENTRY_ADDRESS_SHIFT;

    fn get_active_table(asid: usize) -> PhysicalAddress {
        M::get_active_table(asid)
    }

    fn activate_table(asid: usize, table: VirtualAddress) {
        M::activate_table(asid, table)
    }

    fn invalidate_all() -> crate::Result<()> {
        M::invalidate_all()
    }

    fn invalidate_range(asid: usize, address_range: Range<VirtualAddress>) -> crate::Result<()> {
        M::invalidate_range(asid, address_range)
    }

    fn entry_is_leaf(entry: &Entry<Self>) -> bool
    where
        Self: Sized,
    {
        let entry = unsafe { core::mem::transmute(entry) };
        M::entry_is_leaf(entry)
    }

    fn phys_to_virt(phys: PhysicalAddress) -> VirtualAddress {
        unsafe { VirtualAddress::new(phys.as_raw()) }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Riscv64Sv39;

impl Mode for Riscv64Sv39 {
    type EntryFlags = EntryFlags;

    const PAGE_SIZE: usize = PAGE_SIZE;
    const PAGE_TABLE_LEVELS: usize = 3; // L0, L1, L2
    const PAGE_TABLE_ENTRIES: usize = PAGE_TABLE_ENTRIES;

    const ENTRY_FLAG_DEFAULT_LEAF: Self::EntryFlags = ENTRY_FLAG_DEFAULT_LEAF;
    const ENTRY_FLAG_DEFAULT_TABLE: Self::EntryFlags = ENTRY_FLAG_DEFAULT_TABLE;
    const ENTRY_FLAG_DEFAULT_READ_WRITE: Self::EntryFlags = ENTRY_FLAG_DEFAULT_READ_WRITE;

    const ENTRY_ADDRESS_SHIFT: usize = ENTRY_ADDRESS_SHIFT;

    fn get_active_table(asid: usize) -> PhysicalAddress {
        unsafe { get_active_table(asid) }
    }

    fn activate_table(asid: usize, table: VirtualAddress) {
        unsafe {
            let ppn = table.as_raw() >> 12;
            satp::set(satp::Mode::Sv39, asid, ppn);
        }
    }

    fn invalidate_all() -> crate::Result<()> {
        sbicall::rfence::sfence_vma(0, usize::MAX, 0, 0)?;
        Ok(())
    }

    fn invalidate_range(asid: usize, address_range: Range<VirtualAddress>) -> crate::Result<()> {
        invalidate_address_range(asid, address_range)
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

    fn phys_to_virt(phys: PhysicalAddress) -> VirtualAddress {
        PHYS_OFFSET.add(phys.as_raw())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Riscv64Sv48;

impl Mode for Riscv64Sv48 {
    type EntryFlags = EntryFlags;

    const PAGE_SIZE: usize = PAGE_SIZE;
    const PAGE_TABLE_LEVELS: usize = 4; // L0, L1, L2, L3
    const PAGE_TABLE_ENTRIES: usize = PAGE_TABLE_ENTRIES;

    const ENTRY_FLAG_DEFAULT_LEAF: Self::EntryFlags = ENTRY_FLAG_DEFAULT_LEAF;
    const ENTRY_FLAG_DEFAULT_TABLE: Self::EntryFlags = ENTRY_FLAG_DEFAULT_TABLE;
    const ENTRY_FLAG_DEFAULT_READ_WRITE: Self::EntryFlags = ENTRY_FLAG_DEFAULT_READ_WRITE;

    const ENTRY_ADDRESS_SHIFT: usize = ENTRY_ADDRESS_SHIFT;

    fn get_active_table(asid: usize) -> PhysicalAddress {
        unsafe { get_active_table(asid) }
    }

    fn activate_table(asid: usize, table: VirtualAddress) {
        unsafe {
            let ppn = table.as_raw() >> 12;
            satp::set(satp::Mode::Sv48, asid, ppn);
        }
    }

    fn invalidate_all() -> crate::Result<()> {
        sbicall::rfence::sfence_vma(0, usize::MAX, 0, 0)?;
        Ok(())
    }

    fn invalidate_range(asid: usize, address_range: Range<VirtualAddress>) -> crate::Result<()> {
        invalidate_address_range(asid, address_range)
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

    fn phys_to_virt(phys: PhysicalAddress) -> VirtualAddress {
        PHYS_OFFSET.add(phys.as_raw())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Riscv64Sv57;

impl Mode for Riscv64Sv57 {
    type EntryFlags = EntryFlags;

    const PAGE_SIZE: usize = PAGE_SIZE;
    const PAGE_TABLE_LEVELS: usize = 5; // L0, L1, L2, L3, L4
    const PAGE_TABLE_ENTRIES: usize = PAGE_TABLE_ENTRIES;

    const ENTRY_FLAG_DEFAULT_LEAF: Self::EntryFlags = ENTRY_FLAG_DEFAULT_LEAF;
    const ENTRY_FLAG_DEFAULT_TABLE: Self::EntryFlags = ENTRY_FLAG_DEFAULT_TABLE;
    const ENTRY_FLAG_DEFAULT_READ_WRITE: Self::EntryFlags = ENTRY_FLAG_DEFAULT_READ_WRITE;

    const ENTRY_ADDRESS_SHIFT: usize = ENTRY_ADDRESS_SHIFT;

    fn get_active_table(asid: usize) -> PhysicalAddress {
        unsafe { get_active_table(asid) }
    }

    fn activate_table(asid: usize, table: VirtualAddress) {
        unsafe {
            let ppn = table.0 >> 12;
            satp::set(satp::Mode::Sv57, asid, ppn);
        }
    }

    fn invalidate_all() -> crate::Result<()> {
        sbicall::rfence::sfence_vma(0, usize::MAX, 0, 0)?;
        Ok(())
    }

    fn invalidate_range(asid: usize, address_range: Range<VirtualAddress>) -> crate::Result<()> {
        invalidate_address_range(asid, address_range)
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

    fn phys_to_virt(phys: PhysicalAddress) -> VirtualAddress {
        PHYS_OFFSET.add(phys.as_raw())
    }
}
