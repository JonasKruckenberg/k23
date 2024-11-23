mod emulate;
pub use emulate::EmulateArch;

cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
        pub use riscv64::*;
    }
}

use crate::frame_alloc::FramesIter;
use crate::{BumpAllocator, FrameAllocator, PhysicalAddress, VirtualAddress};
use bitflags::bitflags;
use core::ops::Range;

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    pub struct ArchFlags: u8 {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
    }
}

pub trait Arch: Sized {
    /// Number of usable bits in a virtual address
    const VA_BITS: u32;
    /// The smallest available page size
    const PAGE_SIZE: usize;

    /// The number of levels the page table has
    const PAGE_TABLE_LEVELS: usize;
    /// The number of page table entries in one table
    const PAGE_TABLE_ENTRIES: usize;

    // derived constants
    const PAGE_OFFSET_MASK: usize = Self::PAGE_SIZE - 1;
    /// Number of bits we need to shift an address by to reach the next page
    const PAGE_SHIFT: usize = (Self::PAGE_SIZE - 1).count_ones() as usize;
    /// Number of bits we need to shift an address by to reach the next page table entry
    const PAGE_ENTRY_SHIFT: usize = (Self::PAGE_TABLE_ENTRIES - 1).count_ones() as usize;
    const PAGE_ENTRY_MASK: usize = Self::PAGE_TABLE_ENTRIES - 1;
    const CANONICAL_VA_MASK: usize = (1 << Self::VA_BITS + 1) - 1;

    fn map<F>(
        &mut self,
        virt: Range<VirtualAddress>,
        phys: FramesIter<'_, F, Self>,
        flags: ArchFlags,
    ) -> crate::Result<()>
    where
        F: FrameAllocator<Self>;
    fn map_contiguous(
        &mut self,
        frame_alloc: &mut BumpAllocator<Self>,
        virt: Range<VirtualAddress>,
        phys: Range<PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()>;
    fn remap_contiguous(
        &mut self,
        virt: Range<VirtualAddress>,
        phys: Range<PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()>;
    fn protect(&mut self, virt: Range<VirtualAddress>, flags: ArchFlags) -> crate::Result<()>;
    fn identity_map_contiguous(
        &mut self,
        frame_alloc: &mut BumpAllocator<Self>,
        phys: Range<PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()> {
        let virt = VirtualAddress::new(phys.start.as_raw())..VirtualAddress::new(phys.end.as_raw());
        self.map_contiguous(frame_alloc, virt, phys, flags)
    }
    fn invalidate_all(&mut self) -> crate::Result<()>;
    fn invalidate_range(&mut self, asid: usize, range: Range<VirtualAddress>) -> crate::Result<()>;
    fn activate(&self) -> crate::Result<()>;
}