use crate::paging::PhysicalAddress;
use bitflags::bitflags;
use core::fmt;
use core::fmt::Formatter;

bitflags! {
    #[derive(Debug, Copy, Clone)]
    pub struct PageFlags: usize {
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

/// A page table entry.
pub struct Entry(usize);

impl Entry {
    pub fn flags(&self) -> PageFlags {
        PageFlags::from_bits_truncate(self.0)
    }

    pub fn set_flags(&mut self, flags: PageFlags) {
        self.0 &= !0x3ff; // clear all previous flags
        self.0 |= flags.bits();
    }

    pub fn set_address(&mut self, adress: PhysicalAddress) {
        self.0 |= adress.as_raw() >> 2;
    }

    pub fn address(&self) -> PhysicalAddress {
        unsafe { PhysicalAddress::new((self.0 & !0x3ff) << 2) }
    }
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let rsw = (self.0 & ((1 << 2) - 1) << 8) >> 8;
        let ppn0 = (self.0 & ((1 << 9) - 1) << 10) >> 10;
        let ppn1 = (self.0 & ((1 << 9) - 1) << 19) >> 19;
        let ppn2 = (self.0 & ((1 << 26) - 1) << 28) >> 28;
        let reserved = (self.0 & ((1 << 7) - 1) << 54) >> 54;
        let pbmt = (self.0 & ((1 << 2) - 1) << 61) >> 61;
        let n = (self.0 & ((1 << 1) - 1) << 63) >> 63;

        f.debug_struct("Entry")
            .field("n", &format_args!("{:01b}", n))
            .field("pbmt", &format_args!("{:02b}", pbmt))
            .field("reserved", &format_args!("{:07b}", reserved))
            .field("ppn2", &format_args!("{:026b}", ppn2))
            .field("ppn1", &format_args!("{:09b}", ppn1))
            .field("ppn0", &format_args!("{:09b}", ppn0))
            .field("rsw", &format_args!("{:02b}", rsw))
            .field("flags", &self.flags())
            .finish()
    }
}
