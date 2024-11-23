use crate::frame_alloc::FramesIter;
use crate::{
    AddressRangeExt, Arch, ArchFlags, BumpAllocator, FrameAllocator, PhysicalAddress,
    VirtualAddress,
};
use bitflags::bitflags;
use core::marker::PhantomData;
use core::ops::Range;
use core::ptr::NonNull;
use riscv::satp;
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
        const ACCESSED    = 1 << 6;
        const DIRTY     = 1 << 7;
    }
}

impl From<ArchFlags> for PTEFlags {
    fn from(arch_flags: ArchFlags) -> Self {
        let mut out = Self::VALID | Self::DIRTY | Self::ACCESSED;

        for flag in arch_flags {
            match flag {
                ArchFlags::READ => out.insert(Self::READ),
                ArchFlags::WRITE => out.insert(Self::WRITE),
                ArchFlags::EXECUTE => out.insert(Self::EXECUTE),
                _ => unreachable!(),
            }
        }

        out
    }
}

const PAGE_SIZE: usize = 4096;
const PAGE_TABLE_ENTRIES: usize = 512;

/// On `RiscV` targets the page table entry's physical address bits are shifted 2 bits to the right.
const PTE_PPN_SHIFT: usize = 2;

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
pub struct Riscv64Sv39 {
    asid: usize,
    physmap_base: VirtualAddress,
    root_pgtable: NonNull<PageTableEntry<Self>>,
}
unsafe impl Send for Riscv64Sv39 {}
unsafe impl Sync for Riscv64Sv39 {}

impl Riscv64Sv39 {
    pub fn new(
        frame_alloc: &mut BumpAllocator<Self>,
        asid: usize,
        physmap_base: VirtualAddress,
    ) -> crate::Result<Self> {
        let frame = frame_alloc.allocate_frame()?;
        Ok(Self {
            asid,
            physmap_base,
            root_pgtable: pgtable_ptr_from_phys(frame, physmap_base),
        })
    }

    pub fn from_active(asid: usize, physmap_base: VirtualAddress) -> crate::Result<Self> {
        let satp = satp::read();
        let pgtable_phys = PhysicalAddress(satp.ppn() << 12);
        debug_assert!(pgtable_phys.as_raw() != 0);
        
        Ok(Self {
            asid,
            physmap_base,
            root_pgtable: pgtable_ptr_from_phys(pgtable_phys, physmap_base),
        })
    }
}
impl Arch for Riscv64Sv39 {
    const VA_BITS: u32 = 38;
    const PAGE_SIZE: usize = PAGE_SIZE;
    const PAGE_TABLE_LEVELS: usize = 3; // L0, L1, L2
    const PAGE_TABLE_ENTRIES: usize = PAGE_TABLE_ENTRIES;

    fn map<F>(
        &mut self,
        mut virt: Range<VirtualAddress>,
        mut iter: FramesIter<'_, F, Self>,
        flags: ArchFlags,
    ) -> crate::Result<()>
    where
        F: FrameAllocator<Self>,
    {
        while let Some(range_phys) = iter.next().transpose()? {
            let range_size = range_phys.size();

            let virt_ = virt.start..virt.start.add(range_size);
            virt.start = virt.start.add(range_size);

            debug_assert_eq!(virt_.size(), range_phys.size());
            map_range(
                self.root_pgtable,
                iter.alloc_mut(),
                virt_.start,
                range_phys.start,
                range_phys.size(),
                flags.into(),
                self.physmap_base,
            )?
        }

        Ok(())
    }

    fn map_contiguous(
        &mut self,
        frame_alloc: &mut BumpAllocator<Self>,
        virt: Range<VirtualAddress>,
        phys: Range<PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()> {
        debug_assert_eq!(virt.size(), phys.size());
        map_range(
            self.root_pgtable,
            frame_alloc,
            virt.start,
            phys.start,
            phys.size(),
            flags.into(),
            self.physmap_base,
        )
    }

    fn remap_contiguous(
        &mut self,
        virt: Range<VirtualAddress>,
        phys: Range<PhysicalAddress>,
        flags: ArchFlags,
    ) -> crate::Result<()> {
        remap_range(
            self.root_pgtable,
            virt.start,
            phys.start,
            phys.size(),
            flags.into(),
            self.physmap_base,
        )
    }

    fn protect(&mut self, virt: Range<VirtualAddress>, flags: ArchFlags) -> crate::Result<()> {
        protect_range(
            self.root_pgtable,
            virt.start,
            virt.size(),
            flags.into(),
            self.physmap_base,
        )
    }

    fn invalidate_all(&mut self) -> crate::Result<()> {
        sfence_vma(0, usize::MAX, 0, 0)?;
        Ok(())
    }

    fn invalidate_range(&mut self, asid: usize, range: Range<VirtualAddress>) -> crate::Result<()> {
        invalidate_address_range(asid, range)
    }

    fn activate(&self) -> crate::Result<()> {
        unsafe {
            let ppn = self.root_pgtable.as_ptr() as usize >> 12;
            satp::set(satp::Mode::Sv39, self.asid, ppn);
        }
        Ok(())
    }
}

fn virt_to_vpn<A>(virt: VirtualAddress, vpn_nr: usize) -> usize
where
    A: Arch,
{
    debug_assert!(vpn_nr < A::PAGE_TABLE_LEVELS);
    let index = (virt.as_raw() >> (A::PAGE_SHIFT + vpn_nr * A::PAGE_ENTRY_SHIFT))
        & (A::PAGE_TABLE_ENTRIES - 1);
    debug_assert!(index < A::PAGE_TABLE_ENTRIES);
    index
}

#[repr(transparent)]
struct PageTableEntry<A> {
    bits: usize,
    _m: PhantomData<A>,
}

impl<A> core::fmt::Debug for PageTableEntry<A> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let rsw = (self.bits & ((1 << 2) - 1) << 8) >> 8;
        let ppn0 = (self.bits & ((1 << 9) - 1) << 10) >> 10;
        let ppn1 = (self.bits & ((1 << 9) - 1) << 19) >> 19;
        let ppn2 = (self.bits & ((1 << 26) - 1) << 28) >> 28;
        let reserved = (self.bits & ((1 << 7) - 1) << 54) >> 54;
        let pbmt = (self.bits & ((1 << 2) - 1) << 61) >> 61;
        let n = (self.bits & ((1 << 1) - 1) << 63) >> 63;

        f.debug_struct("Entry")
            .field("n", &format_args!("{n:01b}"))
            .field("pbmt", &format_args!("{pbmt:02b}"))
            .field("reserved", &format_args!("{reserved:07b}"))
            .field("ppn2", &format_args!("{ppn2:026b}"))
            .field("ppn1", &format_args!("{ppn1:09b}"))
            .field("ppn0", &format_args!("{ppn0:09b}"))
            .field("rsw", &format_args!("{rsw:02b}"))
            .field("flags", &self.get_flags())
            .finish()
    }
}

impl<A> PageTableEntry<A> {
    pub fn is_valid(&self) -> bool {
        PTEFlags::from_bits_retain(self.bits).contains(PTEFlags::VALID)
    }
    pub fn is_leaf(&self) -> bool {
        PTEFlags::from_bits_retain(self.bits)
            .intersects(PTEFlags::READ | PTEFlags::WRITE | PTEFlags::EXECUTE)
    }

    pub fn set_address_and_flags(&mut self, address: PhysicalAddress, flags: PTEFlags) {
        self.bits &= PTEFlags::all().bits(); // clear all previous flags
        self.bits |= (address.0 >> PTE_PPN_SHIFT) | flags.bits();
    }

    /// Returns the physical address stored in this page table entry
    ///
    /// This will either be the physical address for page translation or a pointer
    /// to the next sub table.
    pub fn phys_addr(&self) -> PhysicalAddress
    where
        A: Arch,
    {
        // TODO refine this
        PhysicalAddress((self.bits & !PTEFlags::all().bits()) << PTE_PPN_SHIFT)
    }

    pub fn set_flags(&mut self, flags: PTEFlags) {
        self.bits &= PTEFlags::all().bits(); // clear all previous flags
        self.bits |= flags.bits();
    }

    pub fn get_flags(&self) -> PTEFlags {
        PTEFlags::from_bits_retain(self.bits)
    }
}

fn pgtable_ptr_from_phys<A>(
    phys: PhysicalAddress,
    phys_offset: VirtualAddress,
) -> NonNull<PageTableEntry<A>> {
    NonNull::new(phys_offset.add(phys.0).as_raw() as *mut _).unwrap()
}

fn map_range<A>(
    root_ptable: NonNull<PageTableEntry<A>>,
    frame_alloc: &mut dyn FrameAllocator<A>,
    mut virt: VirtualAddress,
    mut phys: PhysicalAddress,
    mut len: usize,
    flags: PTEFlags,
    physmap_base: VirtualAddress,
) -> crate::Result<()>
where
    A: Arch,
{
    'outer: while len > 0 {
        let mut ptable: NonNull<PageTableEntry<A>> = root_ptable;

        for lvl in (0..A::PAGE_TABLE_LEVELS).rev() {
            let index = virt_to_vpn::<A>(virt, lvl);
            let pte = unsafe { &mut *ptable.as_ptr().add(index) };

            if !pte.is_valid() {
                let page_size = 1 << (A::PAGE_SHIFT + lvl * A::PAGE_ENTRY_SHIFT);
                debug_assert!(page_size == 4096 || page_size == 2097152 || page_size == 1073741824);

                // We can use this page size if both virtual and physical address are aligned to it and
                // the remaining size is at least a page size
                if virt.is_aligned(page_size) && phys.is_aligned(page_size) && len >= page_size {
                    pte.set_address_and_flags(phys, flags.union(PTEFlags::VALID));

                    virt = virt.add(page_size);
                    phys = phys.add(page_size);
                    len -= page_size;
                    continue 'outer;
                } else {
                    let frame = frame_alloc.allocate_frame()?;
                    pte.set_address_and_flags(frame, PTEFlags::VALID);
                    ptable = pgtable_ptr_from_phys(frame, physmap_base);
                }
            } else if !pte.is_leaf() {
                ptable = pgtable_ptr_from_phys(pte.phys_addr(), physmap_base);
            } else {
                panic!("must be either free or internal node {pte:?}")
            }
        }
    }

    Ok(())
}

fn remap_range<A>(
    root_ptable: NonNull<PageTableEntry<A>>,
    mut virt: VirtualAddress,
    mut phys: PhysicalAddress,
    mut len: usize,
    flags: PTEFlags,
    physmap_base: VirtualAddress,
) -> crate::Result<()>
where
    A: Arch,
{
    'outer: while len > 0 {
        let mut ptable: NonNull<PageTableEntry<A>> = root_ptable;

        for lvl in (0..A::PAGE_TABLE_LEVELS).rev() {
            let index = virt_to_vpn::<A>(virt, lvl);
            let pte = unsafe { &mut *ptable.as_ptr().add(index) };

            if pte.is_valid() && pte.is_leaf() {
                let page_size = 8 << (A::PAGE_ENTRY_SHIFT * (lvl + 1));
                debug_assert!(page_size == 4096 || page_size == 2097152 || page_size == 1073741824);

                // ensure we can actually map at this alignment and size
                assert!(
                    virt.is_aligned(page_size) && phys.is_aligned(page_size) && len >= page_size
                );
                pte.set_address_and_flags(phys, flags.union(PTEFlags::VALID));

                virt = virt.add(page_size);
                phys = phys.add(page_size);
                len -= page_size;
                continue 'outer;
            } else if pte.is_valid() {
                ptable = pgtable_ptr_from_phys(pte.phys_addr(), physmap_base);
            } else {
                panic!()
            }
        }
    }

    Ok(())
}

fn protect_range<A>(
    root_ptable: NonNull<PageTableEntry<A>>,
    mut virt: VirtualAddress,
    mut len: usize,
    flags: PTEFlags,
    physmap_base: VirtualAddress,
) -> crate::Result<()>
where
    A: Arch,
{
    'outer: while len > 0 {
        let mut ptable: NonNull<PageTableEntry<A>> = root_ptable;

        for lvl in (0..A::PAGE_TABLE_LEVELS).rev() {
            let index = virt_to_vpn::<A>(virt, lvl);
            let pte = unsafe { &mut *ptable.as_ptr().add(index) };

            if pte.is_valid() && pte.is_leaf() {
                let page_size = 8 << (A::PAGE_ENTRY_SHIFT * (lvl + 1));
                debug_assert!(page_size == 4096 || page_size == 2097152 || page_size == 1073741824);

                // sanity check to ensure that we're only operating on aligned slices here
                assert!(virt.is_aligned(page_size) && len >= page_size);
                pte.set_flags(pte.get_flags().union(flags));

                virt = virt.add(page_size);
                len -= page_size;
                continue 'outer;
            } else if pte.is_valid() {
                ptable = pgtable_ptr_from_phys(pte.phys_addr(), physmap_base);
            } else {
                panic!()
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::virt_to_vpn;
    use crate::{EmulateArch, VirtualAddress};

    fn virt_from_parts(
        vpn2: usize,
        vpn1: usize,
        vpn0: usize,
        page_offset: usize,
    ) -> VirtualAddress {
        let raw = ((vpn2 << 30) | (vpn1 << 21) | (vpn0 << 12) | page_offset) as isize;
        let shift = 64 * 8 - 38;
        VirtualAddress(raw.wrapping_shl(shift).wrapping_shr(shift) as usize)
    }

    #[test]
    fn test_virt_to_vpn() {
        let virt = virt_from_parts(1, 2, 3, 4);

        assert_eq!(virt_to_vpn::<EmulateArch>(virt, 2), 1);
        // assert_eq!(virt_to_vpn::<EmulateArch>(virt, 1), 2);
        assert_eq!(virt_to_vpn::<EmulateArch>(virt, 0), 3);
    }
}
