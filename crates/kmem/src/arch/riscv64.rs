use crate::arch::Arch;
use crate::VirtualAddress;
use core::ops::Range;

const PHYS_OFFSET: usize = 0xFFFF_8000_0000_0000;

const PAGE_SIZE: usize = 4096;
const ADDR_PPN_BITS: usize = 9;
const ADDR_OFFSET_BITS: usize = 12;
const ENTRY_FLAGS_MASK: usize = 0x3ff;
const ENTRY_ADDR_SHIFT: usize = 2;

const ENTRY_FLAG_VALID: usize = 1 << 0;
const ENTRY_FLAG_READ: usize = 1 << 1;
const ENTRY_FLAG_WRITE: usize = 1 << 2;
const ENTRY_FLAG_EXECUTE: usize = 1 << 3;
const ENTRY_FLAG_USER: usize = 1 << 4;

fn invalidate_range(
    address_space: usize,
    address_range: Range<VirtualAddress>,
) -> crate::Result<()> {
    let base_addr = address_range.start.as_raw();
    let size = address_range.end.as_raw() - address_range.start.as_raw();
    sbicall::rfence::sfence_vma_asid(0, usize::MAX, base_addr, size, address_space)?;
    Ok(())
}

pub struct Riscv64Sv39;

impl Arch for Riscv64Sv39 {
    const PAGE_SIZE: usize = PAGE_SIZE;
    const VIRT_ADDR_BITS: u32 = 38;
    const PAGE_LEVELS: usize = 3;

    const ENTRY_FLAG_VALID: usize = ENTRY_FLAG_VALID;
    const ENTRY_FLAG_READ: usize = ENTRY_FLAG_READ;
    const ENTRY_FLAG_WRITE: usize = ENTRY_FLAG_WRITE;
    const ENTRY_FLAG_EXECUTE: usize = ENTRY_FLAG_EXECUTE;
    const ENTRY_FLAG_USER: usize = ENTRY_FLAG_USER;

    const ADDR_PPN_BITS: usize = ADDR_PPN_BITS;
    const ADDR_OFFSET_BITS: usize = ADDR_OFFSET_BITS;
    const ENTRY_FLAGS_MASK: usize = ENTRY_FLAGS_MASK;
    const ENTRY_ADDR_SHIFT: usize = ENTRY_ADDR_SHIFT;

    const PHYS_OFFSET: usize = PHYS_OFFSET;

    fn invalidate_all() -> crate::Result<()> {
        sbicall::rfence::sfence_vma(0, usize::MAX, 0, 0)?;
        Ok(())
    }

    fn invalidate_range(
        address_space: usize,
        address_range: Range<VirtualAddress>,
    ) -> crate::Result<()> {
        invalidate_range(address_space, address_range)
    }
}

pub struct Riscv64Sv48;

impl Arch for Riscv64Sv48 {
    const PAGE_SIZE: usize = PAGE_SIZE;
    const VIRT_ADDR_BITS: u32 = 47;
    const PAGE_LEVELS: usize = 4;

    const ENTRY_FLAG_VALID: usize = ENTRY_FLAG_VALID;
    const ENTRY_FLAG_READ: usize = ENTRY_FLAG_READ;
    const ENTRY_FLAG_WRITE: usize = ENTRY_FLAG_WRITE;
    const ENTRY_FLAG_EXECUTE: usize = ENTRY_FLAG_EXECUTE;
    const ENTRY_FLAG_USER: usize = ENTRY_FLAG_USER;

    const ADDR_PPN_BITS: usize = ADDR_PPN_BITS;
    const ADDR_OFFSET_BITS: usize = ADDR_OFFSET_BITS;
    const ENTRY_FLAGS_MASK: usize = ENTRY_FLAGS_MASK;
    const ENTRY_ADDR_SHIFT: usize = ENTRY_ADDR_SHIFT;
    const PHYS_OFFSET: usize = PHYS_OFFSET;

    fn invalidate_all() -> crate::Result<()> {
        sbicall::rfence::sfence_vma(0, usize::MAX, 0, 0)?;
        Ok(())
    }

    fn invalidate_range(
        address_space: usize,
        address_range: Range<VirtualAddress>,
    ) -> crate::Result<()> {
        invalidate_range(address_space, address_range)
    }
}

pub struct Riscv64Sv57;

impl Arch for Riscv64Sv57 {
    const PAGE_SIZE: usize = PAGE_SIZE;
    const VIRT_ADDR_BITS: u32 = 56;
    const PAGE_LEVELS: usize = 5;

    const ENTRY_FLAG_VALID: usize = ENTRY_FLAG_VALID;
    const ENTRY_FLAG_READ: usize = ENTRY_FLAG_READ;
    const ENTRY_FLAG_WRITE: usize = ENTRY_FLAG_WRITE;
    const ENTRY_FLAG_EXECUTE: usize = ENTRY_FLAG_EXECUTE;
    const ENTRY_FLAG_USER: usize = ENTRY_FLAG_USER;

    const ADDR_PPN_BITS: usize = ADDR_PPN_BITS;
    const ADDR_OFFSET_BITS: usize = ADDR_OFFSET_BITS;
    const ENTRY_FLAGS_MASK: usize = ENTRY_FLAGS_MASK;
    const ENTRY_ADDR_SHIFT: usize = ENTRY_ADDR_SHIFT;
    const PHYS_OFFSET: usize = PHYS_OFFSET;

    fn invalidate_all() -> crate::Result<()> {
        sbicall::rfence::sfence_vma(0, usize::MAX, 0, 0)?;
        Ok(())
    }

    fn invalidate_range(
        address_space: usize,
        address_range: Range<VirtualAddress>,
    ) -> crate::Result<()> {
        invalidate_range(address_space, address_range)
    }
}
