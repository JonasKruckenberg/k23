#![no_std]
#![feature(error_in_core)]

mod arch;
mod error;
mod flush;
mod frame_alloc;
mod mapper;
mod table;

use core::cmp::Ordering;
use core::fmt::Formatter;
use core::{fmt, ops};

pub use arch::*;
pub use error::Error;
pub use flush::Flush;
pub use frame_alloc::FrameAllocator;
pub use mapper::Mapper;
pub use table::{Entry, PageFlags};

pub(crate) type Result<T> = core::result::Result<T, Error>;

// TODO implement through global static instead of generics and pass &'static dyn Arch
// or just pass &dyn Arch as param

pub const KIB: usize = 1024;
pub const MIB: usize = 1024 * KIB;
pub const GIB: usize = 1024 * MIB;

/// A physical address.
#[derive(Copy, Clone, PartialOrd, Ord, PartialEq, Eq, Hash)]
pub struct PhysicalAddress(usize);

impl PhysicalAddress {
    pub const unsafe fn new(addr: usize) -> Self {
        Self(addr)
    }

    pub const fn add(&self, offset: usize) -> Self {
        let (out, underflow) = self.0.overflowing_add(offset);
        if underflow {
            panic!("address underflow");
        }
        Self(out)
    }

    pub const fn sub(&self, offset: usize) -> Self {
        let (out, underflow) = self.0.overflowing_sub(offset);
        if underflow {
            panic!("address underflow");
        }
        Self(out)
    }

    pub const fn as_raw(&self) -> usize {
        self.0
    }
}

impl fmt::Debug for PhysicalAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PhysicalAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
        //         f.debug_struct("PhysicalAddress")
        //             .field("page_offset", &get_bits!(self.0, length: 12, offset: 0))
        //             .field("ppn0", &get_bits!(self.0, length: 9, offset: 12))
        //             .field("ppn1", &get_bits!(self.0, length: 9, offset: 21))
        //             .field("ppn2", &get_bits!(self.0, length: 26, offset: 30))
        //             .finish()
    }
}

/// A virtual address.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct VirtualAddress(isize);

impl PartialOrd for VirtualAddress {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.as_raw().partial_cmp(&other.as_raw())
    }
}

impl Ord for VirtualAddress {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_raw().cmp(&other.as_raw())
    }
}

impl VirtualAddress {
    pub const unsafe fn new(addr: usize) -> Self {
        Self(addr as isize)
    }

    pub const fn add(&self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_add_unsigned(offset);
        if overflow {
            panic!("address overflow");
        }
        Self(out)
    }

    pub const fn sub(&self, offset: usize) -> Self {
        let (out, underflow) = self.0.overflowing_sub_unsigned(offset);
        if underflow {
            panic!("address underflow");
        }
        Self(out)
    }

    pub const fn as_raw(&self) -> usize {
        self.0 as usize
    }
}

impl ops::BitOr for VirtualAddress {
    type Output = VirtualAddress;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl fmt::Debug for VirtualAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VirtualAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
        //          f.debug_struct("VirtualAddress")
        //              .field("page_offset", &get_bits!(self.0, length: 12, offset: 0))
        //              .field("vpn0", &self.vpn0())
        //              .field("vpn1", &self.vpn1())
        //              .field("vpn2", &self.vpn2())
        //              .finish()
    }
}
