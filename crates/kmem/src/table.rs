use crate::arch::Arch;
use crate::error::ensure;
use crate::Error;
use crate::{PhysicalAddress, VirtualAddress};
use core::marker::PhantomData;
use core::{mem, ops};

/// This represents a single level of a recursive page table
pub struct Table<A> {
    // The level this table is at, will be arch::MAX level for the root table
    level: usize,
    // The start address of the entries (conceptually it is [Entry; 512])
    addr: PhysicalAddress,
    _m: PhantomData<A>,
}

impl<A: Arch> Table<A> {
    pub fn new(addr: PhysicalAddress, level: usize) -> Self {
        Self {
            level,
            addr,
            _m: PhantomData,
        }
    }

    pub fn level(&self) -> usize {
        self.level
    }

    pub fn address(&self) -> PhysicalAddress {
        self.addr
    }

    pub fn index_of_virt(&self, virt: VirtualAddress) -> usize {
        // A virtual address is made up of a 12-bit page offset and 3 9-bit page numbers
        //
        // We therefore need to right-shift first by 12 bits to account for the page offset
        // and then level * 9 bits to get the correct page number
        // we then mask out all bits but the 9 we are interested in
        (virt.as_raw() >> (self.level * A::ADDR_PPN_BITS + A::ADDR_OFFSET_BITS)) & A::ADDR_PPN_MASK
    }

    pub fn virt_from_index(&self, index: usize) -> VirtualAddress {
        // TODO make sure addr is valid
        unsafe {
            VirtualAddress::new(
                (index & A::ADDR_PPN_MASK) << (self.level * A::ADDR_PPN_BITS + A::ADDR_OFFSET_BITS),
            )
        }
    }

    pub fn entry(&self, index: usize) -> crate::Result<&Entry<A>> {
        ensure!(index < 512, Error::PageIndexOutOfBounds(index));

        let ptr = self.addr.add(index * mem::size_of::<Entry<A>>()).as_raw() as *const Entry<A>;

        Ok(unsafe { &*(ptr) })
    }

    pub fn entry_mut(&mut self, index: usize) -> crate::Result<&mut Entry<A>> {
        ensure!(index < 512, Error::PageIndexOutOfBounds(index));

        let ptr = self.addr.add(index * mem::size_of::<Entry<A>>()).as_raw() as *mut Entry<A>;

        Ok(unsafe { &mut *(ptr) })
    }

    // pub fn lowest_mapped_address(&self) -> crate::Result<VirtualAddress> {
    //     self.lowest_mapped_address_inner(unsafe { VirtualAddress::new(0) })
    // }
    //
    // pub fn lowest_mapped_address_inner(
    //     &self,
    //     acc: VirtualAddress,
    // ) -> crate::Result<VirtualAddress> {
    //     for i in 0..512 {
    //         let entry = self.entry(i)?;
    //         let virt = acc | self.virt_from_index(i);
    //
    //         if entry
    //             .flags()
    //             .intersects(PageFlags::READ | PageFlags::EXECUTE)
    //         {
    //             return Ok(virt);
    //         } else if entry.is_valid() {
    //             return Self::new(entry.address(), self.level - 1)
    //                 .lowest_mapped_address_inner(virt);
    //         }
    //     }
    //
    //     todo!()
    // }
    //
    // pub fn highest_mapped_address(&self) -> crate::Result<VirtualAddress> {
    //     self.highest_mapped_address_inner(unsafe { VirtualAddress::new(0) })
    // }
    //
    // fn highest_mapped_address_inner(&self, acc: VirtualAddress) -> crate::Result<VirtualAddress> {
    //     for i in (0..512).rev() {
    //         let entry = self.entry(i)?;
    //         let virt = acc | self.virt_from_index(i);
    //
    //         if entry
    //             .flags()
    //             .intersects(PageFlags::READ | PageFlags::EXECUTE)
    //         {
    //             return Ok(virt);
    //         } else if entry.is_valid() {
    //             return Self::new(entry.address(), self.level - 1)
    //                 .highest_mapped_address_inner(virt);
    //         }
    //     }
    //
    //     todo!()
    // }
    //
    // #[cfg(debug_assertions)]
    // pub fn debug_print_table(&self) -> crate::Result<()> {
    //     self.debug_print_table_inner(unsafe { VirtualAddress::new(0) })
    // }
    //
    // #[cfg(debug_assertions)]
    // fn debug_print_table_inner(&self, acc: VirtualAddress) -> crate::Result<()> {
    //     let padding = match self.level {
    //         0 => 8,
    //         1 => 4,
    //         _ => 0,
    //     };
    //
    //     for i in 0..512 {
    //         let entry = &self.entry(i)?;
    //         let virt = acc | self.virt_from_index(i);
    //
    //         if entry
    //             .flags()
    //             .intersects(PageFlags::READ | PageFlags::EXECUTE)
    //         {
    //             log::debug!(
    //                 "{:^padding$}{}:{i} is a leaf {} => {}",
    //                 "",
    //                 self.level,
    //                 virt,
    //                 entry.address(),
    //             );
    //         } else if entry.is_valid() {
    //             log::debug!("{:^padding$}{}:{i} is a table node", "", self.level);
    //             Self::new(entry.address(), self.level - 1).debug_print_table_inner(virt)?;
    //         }
    //     }
    //
    //     Ok(())
    // }
}

#[derive(Debug)]
#[repr(transparent)]
pub struct Entry<A> {
    inner: usize,
    _m: PhantomData<A>,
}

impl<A: Arch> Entry<A> {
    pub fn flags(&self) -> PageFlags<A> {
        PageFlags::from_bits_truncate(self.inner)
    }

    pub fn set_flags(&mut self, flags: PageFlags<A>) {
        self.inner &= !0x3ff; // clear all previous flags
        self.inner |= flags.bits;
    }

    pub fn is_valid(&self) -> bool {
        self.flags().intersects(PageFlags::VALID)
    }

    pub fn address(&self) -> PhysicalAddress {
        unsafe { PhysicalAddress::new((self.inner & !A::ENTRY_FLAGS_MASK) << A::ENTRY_ADDR_SHIFT) }
    }

    pub fn set_address(&mut self, address: PhysicalAddress) {
        self.inner |= address.as_raw() >> A::ENTRY_ADDR_SHIFT;
    }

    pub fn clear(&mut self) {
        self.inner = 0;
    }
}

pub struct PageFlags<A> {
    bits: usize,
    _m: PhantomData<A>,
}

impl<A> Clone for PageFlags<A> {
    fn clone(&self) -> Self {
        Self {
            bits: self.bits,
            _m: PhantomData,
        }
    }
}

impl<A> Copy for PageFlags<A> {}

impl<A> PageFlags<A> {
    pub const fn from_bits_retain(bits: usize) -> Self {
        Self {
            bits,
            _m: PhantomData,
        }
    }
}

impl<A: Arch> PageFlags<A> {
    pub const fn from_bits_truncate(bits: usize) -> Self {
        Self {
            bits: bits & A::ENTRY_FLAGS_MASK,
            _m: PhantomData,
        }
    }

    pub const VALID: Self = Self::from_bits_retain(A::ENTRY_FLAG_VALID);
    pub const READ: Self = Self::from_bits_retain(A::ENTRY_FLAG_READ);
    pub const WRITE: Self = Self::from_bits_retain(A::ENTRY_FLAG_WRITE);
    pub const EXECUTE: Self = Self::from_bits_retain(A::ENTRY_FLAG_EXECUTE);
    pub const USER: Self = Self::from_bits_retain(A::ENTRY_FLAG_USER);

    /// Whether *any* set bits in a source flags value are also set in a target flags value.
    #[inline]
    pub fn intersects(&self, other: Self) -> bool {
        self.bits & other.bits != 0
    }
}

impl<A> ops::BitOr for PageFlags<A> {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self::from_bits_retain(self.bits | rhs.bits)
    }
}
