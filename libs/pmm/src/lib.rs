#![feature(let_chains)]
#![feature(debug_closure_helpers)]
#![no_std]

mod address_space;
pub mod arch;
mod error;
mod flush;
pub mod frame_alloc;

use core::fmt;

pub use address_space::AddressSpace;
pub use error::Error;
pub use flush::Flush;
pub(crate) type Result<T> = core::result::Result<T, Error>;

pub const KIB: usize = 1024;
pub const MIB: usize = 1024 * KIB;
pub const GIB: usize = 1024 * MIB;

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq)]
    pub struct Flags: u8 {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
    }
}

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VirtualAddress(usize);
impl VirtualAddress {
    #[must_use]
    pub const fn new(bits: usize) -> Self {
        debug_assert!(bits != 0);
        Self(bits)
    }

    #[must_use]
    #[inline]
    pub const fn from_phys(phys: PhysicalAddress, physmap_offset: VirtualAddress) -> Self {
        physmap_offset.add(phys.as_raw())
    }

    #[must_use]
    #[inline]
    #[allow(clippy::cast_sign_loss)]
    pub const fn offset(self, offset: isize) -> Self {
        if offset.is_negative() {
            self.sub(offset.wrapping_abs() as usize)
        } else {
            self.add(offset as usize)
        }
    }

    #[must_use]
    #[inline]
    pub const fn add(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_add(offset);
        assert!(!overflow, "virtual address overflow");
        Self(out)
    }

    #[must_use]
    #[inline]
    pub const fn sub(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_sub(offset);
        assert!(!overflow, "virtual address overflow");
        Self(out)
    }

    #[must_use]
    #[inline]
    pub const fn sub_addr(self, rhs: Self) -> usize {
        let (out, overflow) = self.0.overflowing_sub(rhs.0);
        assert!(!overflow, "virtual address underflow");
        out
    }

    #[must_use]
    #[inline]
    pub const fn as_raw(&self) -> usize {
        self.0
    }

    #[must_use]
    #[inline]
    pub const fn is_aligned(&self, align: usize) -> bool {
        assert!(
            align.is_power_of_two(),
            "is_aligned_to: align is not a power-of-two"
        );

        self.as_raw() & (align - 1) == 0
    }

    #[must_use]
    #[inline]
    pub const fn align_down(self, alignment: usize) -> Self {
        Self(self.0 & !(alignment - 1))
    }

    #[must_use]
    #[inline]
    pub const fn align_up(self, alignment: usize) -> Self {
        Self((self.0 + alignment - 1) & !(alignment - 1))
    }
}
impl fmt::Display for VirtualAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{:#x}", self.0))
    }
}
impl fmt::Debug for VirtualAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VirtualAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PhysicalAddress(usize);
impl PhysicalAddress {
    #[must_use]
    #[inline]
    pub const fn new(bits: usize) -> Self {
        debug_assert!(bits != 0);
        Self(bits)
    }

    #[must_use]
    #[inline]
    #[allow(clippy::cast_sign_loss)]
    pub const fn offset(self, offset: isize) -> Self {
        if offset.is_negative() {
            self.sub(offset.wrapping_abs() as usize)
        } else {
            self.add(offset as usize)
        }
    }

    #[must_use]
    #[inline]
    pub const fn add(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_add(offset);
        assert!(!overflow, "physical address overflow");
        Self(out)
    }

    #[must_use]
    #[inline]
    pub const fn sub(self, offset: usize) -> Self {
        let (out, overflow) = self.0.overflowing_sub(offset);
        assert!(!overflow, "physical address underflow");
        Self(out)
    }

    #[must_use]
    #[inline]
    pub const fn sub_addr(self, rhs: Self) -> usize {
        let (out, overflow) = self.0.overflowing_sub(rhs.0);
        assert!(!overflow, "physical address underflow");
        out
    }

    #[must_use]
    #[inline]
    pub const fn as_raw(&self) -> usize {
        self.0
    }

    #[must_use]
    #[inline]
    pub const fn is_aligned(&self, align: usize) -> bool {
        assert!(
            align.is_power_of_two(),
            "is_aligned_to: align is not a power-of-two"
        );

        self.as_raw() & (align - 1) == 0
    }

    #[must_use]
    #[inline]
    pub const fn align_down(self, alignment: usize) -> Self {
        Self(self.0 & !(alignment - 1))
    }

    #[must_use]
    #[inline]
    pub const fn align_up(self, alignment: usize) -> Self {
        Self((self.0 + alignment - 1) & !(alignment - 1))
    }
}
impl fmt::Display for PhysicalAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{:#x}", self.0))
    }
}
impl fmt::Debug for PhysicalAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PhysicalAddress")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}
