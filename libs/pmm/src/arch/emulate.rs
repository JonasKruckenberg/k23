use crate::{Arch, ArchFlags, BumpAllocator, FrameAllocator, PhysicalAddress, VirtualAddress};
use core::ops::Range;
use crate::frame_alloc::FramesIter;

macro_rules! get_bits {
    ($num: expr, length: $length: expr, offset: $offset: expr) => {
        ($num & (((1 << $length) - 1) << $offset)) >> $offset
    };
}

/// Mock `RiscvSv39` architecture for testing
pub struct EmulateArch;

impl EmulateArch {
    #[must_use]
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
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

    #[must_use]
    pub fn virt_into_parts(virt: VirtualAddress) -> (usize, usize, usize, usize) {
        let vpn2 = get_bits!(virt.0, length: 9, offset: 30);
        let vpn1 = get_bits!(virt.0, length: 9, offset: 21);
        let vpn0 = get_bits!(virt.0, length: 9, offset: 12);
        let offset = virt.0 & Self::PAGE_OFFSET_MASK;
        (vpn2, vpn1, vpn0, offset)
    }
}

impl Arch for EmulateArch {
    const VA_BITS: u32 = 38;
    const PAGE_SIZE: usize =4096;
    const PAGE_TABLE_LEVELS: usize = 3; // L0, L1, L2
    const PAGE_TABLE_ENTRIES: usize = 512;

    fn map<F>(
        &mut self,
        mut virt: Range<VirtualAddress>,
        mut iter: FramesIter<'_, F, Self>,
        flags: ArchFlags,
    ) -> crate::Result<()>
    where
        F: FrameAllocator<Self>,
    {
        todo!()
    }

    fn map_contiguous(
        &mut self,
        frame_alloc: &mut BumpAllocator<Self>,
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

    fn protect(&mut self, virt: Range<VirtualAddress>, flags: ArchFlags) -> crate::Result<()> {
        todo!()
    }

    fn invalidate_all(&mut self) -> crate::Result<()> {
        Ok(())
    }

    fn invalidate_range(
        &mut self,
        _asid: usize,
        _address_range: Range<VirtualAddress>,
    ) -> crate::Result<()> {
        Ok(())
    }

    fn activate(&self) -> crate::Result<()> {
        todo!()
    }
}
