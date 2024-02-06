use core::fmt;
use core::fmt::Formatter;
macro_rules! get_bits {
    ($num: expr, length: $length: expr, offset: $offset: expr) => {
        ($num & (((1 << $length) - 1) << $offset)) >> $offset
    };
}

/// A physical address.
#[derive(Copy, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct PhysicalAddress(usize);

impl PhysicalAddress {
    pub const unsafe fn new(addr: usize) -> Self {
        Self(addr)
    }

    pub const fn add(&self, offset: usize) -> Self {
        Self(self.0 + offset)
    }

    pub const fn sub(&self, offset: usize) -> Self {
        Self(self.0 - offset)
    }

    pub const fn as_raw(&self) -> usize {
        self.0
    }
}

impl fmt::Display for PhysicalAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PhysicalAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

impl fmt::Debug for PhysicalAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("PhysicalAddress")
            .field("page_offset", &get_bits!(self.0, length: 12, offset: 0))
            .field("ppn0", &get_bits!(self.0, length: 9, offset: 12))
            .field("ppn1", &get_bits!(self.0, length: 9, offset: 21))
            .field("ppn2", &get_bits!(self.0, length: 26, offset: 30))
            .finish()
    }
}

/// A virtual address.
#[derive(Copy, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct VirtualAddress(isize);

impl VirtualAddress {
    pub const unsafe fn new(addr: usize) -> Self {
        let shift = 64 * 8 - 38;

        Self((addr as isize).wrapping_shl(shift).wrapping_shr(shift))
    }

    pub const unsafe fn from_raw_parts(
        vpn2: usize,
        vpn1: usize,
        vpn0: usize,
        page_offset: usize,
    ) -> Self {
        Self::new(vpn2 << 30 | vpn1 << 21 | vpn0 << 12 | page_offset)
    }

    pub fn vpn2(&self) -> usize {
        get_bits!(self.0, length: 9, offset: 30) as usize
    }

    pub fn vpn1(&self) -> usize {
        get_bits!(self.0, length: 9, offset: 21) as usize
    }

    pub fn vpn0(&self) -> usize {
        get_bits!(self.0, length: 9, offset: 12) as usize
    }

    pub const fn add(&self, offset: usize) -> Self {
        Self(self.0.saturating_add_unsigned(offset))
    }

    pub const fn sub(&self, offset: usize) -> Self {
        Self(self.0.saturating_sub_unsigned(offset))
    }

    pub const fn as_raw(&self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for VirtualAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VirtualAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

impl fmt::Debug for VirtualAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("VirtualAddress")
            .field("page_offset", &get_bits!(self.0, length: 12, offset: 0))
            .field("vpn0", &self.vpn0())
            .field("vpn1", &self.vpn1())
            .field("vpn2", &self.vpn2())
            .finish()
    }
}
