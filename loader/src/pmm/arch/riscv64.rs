use crate::pmm::{Error, Flags, FrameAllocator, FramesIter, PhysicalAddress, VirtualAddress};
use bitflags::bitflags;
use core::num::NonZeroUsize;
use core::ptr::NonNull;

/// Number of usable bits in a virtual address
pub const VA_BITS: u32 = 38;
/// The smallest available page size
pub const PAGE_SIZE: usize = 4096;

/// The number of levels the page table has
pub const PAGE_TABLE_LEVELS: usize = 3; // L0, L1, L2
/// The number of page table entries in one table
pub const PAGE_TABLE_ENTRIES: usize = 512;

// derived constants
/// Number of bits we need to shift an address by to reach the next page
pub const PAGE_SHIFT: usize = (PAGE_SIZE - 1).count_ones() as usize;
/// Number of bits we need to shift an address by to reach the next page table entry
pub const PAGE_ENTRY_SHIFT: usize = (PAGE_TABLE_ENTRIES - 1).count_ones() as usize;

/// On `RiscV` targets the page table entry's physical address bits are shifted 2 bits to the right.
pub const PTE_PPN_SHIFT: usize = 2;

// Virtual address where the kernel address space begins.
// Below this is the user address space.
// riscv64 with sv39 means a page-based 39-bit virtual memory space.  The
// base kernel address is chosen so that kernel addresses have a 1 in the
// most significant bit whereas user addresses have a 0. 
pub const KERNEL_ASPACE_BASE: VirtualAddress = VirtualAddress::new(usize::MAX << VA_BITS); // 0xffffffc000000000

pub const fn page_size_for_level(lvl: usize) -> usize {
    let page_size = 1 << (PAGE_SHIFT + lvl * PAGE_ENTRY_SHIFT);
    debug_assert!(page_size == 4096 || page_size == 2097152 || page_size == 1073741824);
    page_size
}

bitflags! {
    #[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
    struct PTEFlags: usize {
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

impl From<Flags> for PTEFlags {
    fn from(arch_flags: Flags) -> Self {
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

pub struct Riscv64Sv39 {
    root_pgtable: NonNull<PageTableEntry>,
    phys_offset: VirtualAddress,
}

impl Riscv64Sv39 {
    pub fn new(
        frame_alloc: &mut dyn FrameAllocator,
        phys_offset: VirtualAddress,
    ) -> Result<Self, Error> {
        let root_pgtable = pgtable_ptr_from_phys(frame_alloc.allocate_frame_zeroed()?, phys_offset);

        Ok(Self {
            root_pgtable,
            phys_offset,
        })
    }

    pub fn map(
        &mut self,
        mut virt: VirtualAddress,
        mut iter: FramesIter,
        flags: Flags,
    ) -> Result<(), Error> {
        while let Some((phys, len)) = iter.next().transpose()? {
            self.map_contiguous(iter.alloc_mut(), virt, phys, len, flags)?;
            virt = virt.add(len.get() * PAGE_SIZE);
        }

        Ok(())
    }

    pub fn map_contiguous(
        &mut self,
        frame_alloc: &mut dyn FrameAllocator,
        mut virt: VirtualAddress,
        mut phys: PhysicalAddress,
        len: NonZeroUsize,
        flags: Flags,
    ) -> Result<(), Error> {
        let mut len = len.get();

        'outer: while len > 0 {
            let mut ptable: NonNull<PageTableEntry> = self.root_pgtable;

            for lvl in (0..PAGE_TABLE_LEVELS).rev() {
                let index = virt_to_vpn(virt, lvl);
                let pte = unsafe { &mut *ptable.as_ptr().add(index) };

                if !pte.is_valid() {
                    let page_size = page_size_for_level(lvl);

                    // We can use this page size if both virtual and physical address are aligned to it and
                    // the remaining size is at least a page size
                    if virt.is_aligned(page_size) && phys.is_aligned(page_size) && len >= page_size
                    {
                        pte.set_address_and_flags(phys, PTEFlags::VALID | flags.into());

                        virt = virt.add(page_size);
                        phys = phys.add(page_size);
                        len -= page_size;
                        continue 'outer;
                    } else {
                        let frame = frame_alloc.allocate_frame_zeroed()?;
                        pte.set_address_and_flags(frame, PTEFlags::VALID);
                        ptable = pgtable_ptr_from_phys(frame, self.phys_offset);
                    }
                } else if !pte.is_leaf() {
                    ptable = pgtable_ptr_from_phys(pte.phys_addr(), self.phys_offset);
                } else {
                    panic!("must be either free or internal node {pte:?}")
                }
            }
        }

        Ok(())
    }

    pub fn protect(
        &mut self,
        mut virt: VirtualAddress,
        len: NonZeroUsize,
        flags: Flags,
    ) -> Result<(), Error> {
        let mut len = len.get();

        'outer: while len > 0 {
            let mut ptable: NonNull<PageTableEntry> = self.root_pgtable;

            for lvl in (0..PAGE_TABLE_LEVELS).rev() {
                let index = virt_to_vpn(virt, lvl);
                let pte = unsafe { &mut *ptable.as_ptr().add(index) };

                if pte.is_valid() && pte.is_leaf() {
                    let page_size = page_size_for_level(lvl);

                    // sanity check to ensure that we're only operating on aligned slices here
                    assert!(virt.is_aligned(page_size) && len >= page_size);
                    pte.set_flags(pte.get_flags() | flags.into());

                    virt = virt.add(page_size);
                    len -= page_size;
                    continue 'outer;
                } else if pte.is_valid() {
                    ptable = pgtable_ptr_from_phys(pte.phys_addr(), self.phys_offset);
                } else {
                    panic!()
                }
            }
        }

        Ok(())
    }
}

#[repr(transparent)]
struct PageTableEntry {
    bits: usize,
}

impl core::fmt::Debug for PageTableEntry {
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

impl PageTableEntry {
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
    pub fn phys_addr(&self) -> PhysicalAddress {
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

fn pgtable_ptr_from_phys(
    phys: PhysicalAddress,
    phys_offset: VirtualAddress,
) -> NonNull<PageTableEntry> {
    NonNull::new(phys_offset.add(phys.0).as_raw() as *mut _).unwrap()
}

fn virt_to_vpn(virt: VirtualAddress, vpn_nr: usize) -> usize {
    debug_assert!(vpn_nr < PAGE_TABLE_LEVELS);
    let index =
        (virt.as_raw() >> (PAGE_SHIFT + vpn_nr * PAGE_ENTRY_SHIFT)) & (PAGE_TABLE_ENTRIES - 1);
    debug_assert!(index < PAGE_TABLE_ENTRIES);
    index
}
