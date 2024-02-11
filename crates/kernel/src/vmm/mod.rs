//! Virtual Memory Management

mod allocator;
mod entry;
mod flush;
mod mapper;
mod mode;
mod table;

pub use allocator::{BitMapAllocator, BumpAllocator, FrameAllocator, FrameUsage};
use core::fmt;
use core::fmt::Formatter;
pub use flush::Flush;
pub use mapper::Mapper;
pub use mode::*;

#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PhysicalAddress(usize);

impl PhysicalAddress {
    pub const unsafe fn new(bits: usize) -> Self {
        Self(bits)
    }

    pub const fn add(&self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_add(offset);
        if overflow {
            panic!("physical address overflow");
        }
        Self(out)
    }

    pub const fn sub(&self, offset: usize) -> Self {
        let (out, underflow) = self.0.overflowing_sub(offset);
        if underflow {
            panic!("physical address underflow");
        }
        Self(out)
    }

    pub const fn as_raw(&self) -> usize {
        self.0
    }
}

impl fmt::Display for PhysicalAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{:#x}", self.0))
    }
}

impl fmt::Debug for PhysicalAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PhysicalAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VirtualAddress(usize);

impl VirtualAddress {
    pub const unsafe fn new(bits: usize) -> Self {
        Self(bits)
    }

    pub const fn add(&self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_add(offset);
        if overflow {
            panic!("virtual address overflow");
        }
        Self(out)
    }

    pub const fn sub(&self, offset: usize) -> Self {
        let (out, underflow) = self.0.overflowing_sub(offset);
        if underflow {
            panic!("virtual address underflow");
        }
        Self(out)
    }

    pub const fn as_raw(&self) -> usize {
        self.0
    }
}

impl fmt::Display for VirtualAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{:#x}", self.0))
    }
}

impl fmt::Debug for VirtualAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VirtualAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}
