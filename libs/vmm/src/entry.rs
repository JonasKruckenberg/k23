use crate::{Mode, PhysicalAddress};
use bitflags::Flags;
use core::marker::PhantomData;

pub struct Entry<M> {
    bits: usize,
    _m: PhantomData<M>,
}

#[cfg(target_arch = "riscv64")]
impl<M: Mode> core::fmt::Debug for Entry<M> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let rsw = (self.bits & ((1 << 2) - 1) << 8) >> 8;
        let ppn0 = (self.bits & ((1 << 9) - 1) << 10) >> 10;
        let ppn1 = (self.bits & ((1 << 9) - 1) << 19) >> 19;
        let ppn2 = (self.bits & ((1 << 26) - 1) << 28) >> 28;
        let reserved = (self.bits & ((1 << 7) - 1) << 54) >> 54;
        let pbmt = (self.bits & ((1 << 2) - 1) << 61) >> 61;
        let n = (self.bits & ((1 << 1) - 1) << 63) >> 63;

        f.debug_struct("Entry")
            .field("n", &format_args!("{:01b}", n))
            .field("pbmt", &format_args!("{:02b}", pbmt))
            .field("reserved", &format_args!("{:07b}", reserved))
            .field("ppn2", &format_args!("{:026b}", ppn2))
            .field("ppn1", &format_args!("{:09b}", ppn1))
            .field("ppn0", &format_args!("{:09b}", ppn0))
            .field("rsw", &format_args!("{:02b}", rsw))
            .field("flags", &self.get_flags())
            .finish()
    }
}

impl<M: Mode> Entry<M> {
    /// Whether this page table entry is vacant, i.e. is neither a leaf nor a table
    ///
    /// All page table entries start out as vacant, becoming filled when their respective
    /// memory regions become mapped. When their respective regions get unmapped, the entry
    /// is returned to a vacant state through [`Entry::clear`]
    pub fn is_vacant(&self) -> bool {
        // all supported architectures used the lowest bit to signal the presence of a valid entry
        // on x86 this is called the "present bit"
        // on RiscV and arm it's called the "valid bit"
        (self.bits & 0b1) == 0
    }

    /// Clears all data stored in this entry and returns it into a vacant state.
    pub fn clear(&mut self) {
        self.bits = 0;
    }

    /// Updates the address and flags of this entry at once
    pub fn set_address_and_flags(&mut self, address: PhysicalAddress, flags: M::EntryFlags) {
        // #[cfg(target_arch = "riscv64")]
        // log::trace!("{self:?}");
        self.bits &= M::EntryFlags::all().into(); // clear all previous flags
        self.bits |= (address.0 >> M::ENTRY_ADDRESS_SHIFT) | flags.into();
    }

    /// Returns the architecture-specific flags for this page table entry
    pub fn get_flags(&self) -> M::EntryFlags {
        M::EntryFlags::from(self.bits)
    }

    /// Returns the physical address stored in this page table entry
    ///
    /// This will either be the physical address for page translation or a pointer
    /// to the next sub table.
    pub fn get_address(&self) -> PhysicalAddress {
        PhysicalAddress((self.bits & !M::EntryFlags::all().into()) << M::ENTRY_ADDRESS_SHIFT)
    }
}
