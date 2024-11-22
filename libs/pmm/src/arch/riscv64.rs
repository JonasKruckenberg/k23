use crate::{Arch, ArchFlags, PhysicalAddress, VirtualAddress};
use bitflags::bitflags;
use core::ops::Range;
use riscv::sbi::rfence::{sfence_vma, sfence_vma_asid};

bitflags! {
    #[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
    pub struct PTEFlags: usize {
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

impl From<usize> for PTEFlags {
    fn from(value: usize) -> Self {
        Self::from_bits_truncate(value)
    }
}

impl From<PTEFlags> for usize {
    fn from(value: PTEFlags) -> Self {
        value.bits()
    }
}

const PAGE_SIZE: usize = 4096;
const PAGE_TABLE_ENTRIES: usize = 512;
const ENTRY_ADDRESS_SHIFT: usize = 2;

fn invalidate_address_range(
    asid: usize,
    address_range: Range<VirtualAddress>,
) -> crate::Result<()> {
    let base_addr = address_range.start.0;
    let size = address_range.end.0 - address_range.start.0;
    sfence_vma_asid(0, usize::MAX, base_addr, size, asid)?;
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub struct Riscv64Sv39;
impl Arch for Riscv64Sv39 {
    const VA_BITS: u32 = 38;
    const PAGE_SIZE: usize = PAGE_SIZE;
    const PAGE_TABLE_LEVELS: usize = 3; // L0, L1, L2
    const PAGE_TABLE_ENTRIES: usize = PAGE_TABLE_ENTRIES;
    const ENTRY_ADDRESS_SHIFT: usize = ENTRY_ADDRESS_SHIFT;

    fn map(
        &mut self,
        virt: VirtualAddress,
        phys: &mut dyn Iterator<Item = PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()> {
        todo!()
    }

    fn map_contiguous(
        &mut self,
        virt: Range<VirtualAddress>,
        phys: Range<PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()> {
        todo!()
    }

    fn remap_contiguous(
        &mut self,
        virt: Range<VirtualAddress>,
        phys: Range<PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()> {
        todo!()
    }

    fn invalidate_all(&mut self) -> crate::Result<()> {
        sfence_vma(0, usize::MAX, 0, 0)?;
        Ok(())
    }

    fn invalidate_range(&mut self, asid: usize, range: Range<VirtualAddress>) -> crate::Result<()> {
        invalidate_address_range(asid, range)
    }

    fn protect(&mut self, virt: Range<VirtualAddress>, flags: ArchFlags) -> crate::Result<()> {
        todo!()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Riscv64Sv48;
impl Arch for Riscv64Sv48 {
    const VA_BITS: u32 = 47;
    const PAGE_SIZE: usize = PAGE_SIZE;
    const PAGE_TABLE_LEVELS: usize = 4; // L0, L1, L2, L3
    const PAGE_TABLE_ENTRIES: usize = PAGE_TABLE_ENTRIES;
    const ENTRY_ADDRESS_SHIFT: usize = ENTRY_ADDRESS_SHIFT;

    fn map(
        &mut self,
        virt: VirtualAddress,
        phys: &mut dyn Iterator<Item = PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()> {
        todo!()
    }

    fn map_contiguous(
        &mut self,
        virt: Range<VirtualAddress>,
        phys: Range<PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()> {
        todo!()
    }

    fn remap_contiguous(
        &mut self,
        virt: Range<VirtualAddress>,
        phys: Range<PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()> {
        todo!()
    }

    fn invalidate_all(&mut self) -> crate::Result<()> {
        sfence_vma(0, usize::MAX, 0, 0)?;
        Ok(())
    }

    fn invalidate_range(&mut self, asid: usize, range: Range<VirtualAddress>) -> crate::Result<()> {
        invalidate_address_range(asid, range)
    }

    fn protect(&mut self, virt: Range<VirtualAddress>, flags: ArchFlags) -> crate::Result<()> {
        todo!()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Riscv64Sv57;
impl Arch for Riscv64Sv57 {
    const VA_BITS: u32 = 56;
    const PAGE_SIZE: usize = PAGE_SIZE;
    const PAGE_TABLE_LEVELS: usize = 5; // L0, L1, L2, L3, L4
    const PAGE_TABLE_ENTRIES: usize = PAGE_TABLE_ENTRIES;
    const ENTRY_ADDRESS_SHIFT: usize = ENTRY_ADDRESS_SHIFT;

    fn map(
        &mut self,
        virt: VirtualAddress,
        phys: &mut dyn Iterator<Item = PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()> {
        todo!()
    }

    fn map_contiguous(
        &mut self,
        virt: Range<VirtualAddress>,
        phys: Range<PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()> {
        todo!()
    }

    fn remap_contiguous(
        &mut self,
        virt: Range<VirtualAddress>,
        phys: Range<PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()> {
        todo!()
    }

    fn invalidate_all(&mut self) -> crate::Result<()> {
        sfence_vma(0, usize::MAX, 0, 0)?;
        Ok(())
    }

    fn invalidate_range(&mut self, asid: usize, range: Range<VirtualAddress>) -> crate::Result<()> {
        invalidate_address_range(asid, range)
    }

    fn protect(&mut self, virt: Range<VirtualAddress>, flags: ArchFlags) -> crate::Result<()> {
        todo!()
    }
}
