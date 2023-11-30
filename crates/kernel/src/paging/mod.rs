use core::fmt;
use core::fmt::Formatter;

pub mod frame_alloc;

/// A physical address.
#[derive(Copy, Clone, PartialOrd, PartialEq)]
pub struct PhysicalAddress(usize);

impl PhysicalAddress {
    pub unsafe fn new(addr: usize) -> Self {
        Self(addr)
    }
    pub fn add(&self, offset: usize) -> Self {
        Self(self.0 + offset)
    }
    pub fn as_raw(&self) -> usize {
        self.0
    }
}

impl fmt::Debug for PhysicalAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PhysicalAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

/// A virtual address.
#[derive(Copy, Clone, PartialOrd, PartialEq)]
pub struct VirtualAddress(usize);

impl VirtualAddress {
    pub unsafe fn new(addr: usize) -> Self {
        Self(addr)
    }

    pub fn as_raw(&self) -> usize {
        self.0
    }
}

impl fmt::Debug for VirtualAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VirtualAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}
