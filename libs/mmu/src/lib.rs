#![feature(let_chains)]
#![feature(debug_closure_helpers)]
#![cfg_attr(test, feature(used_with_arg))]
#![cfg_attr(test, no_main)]
#![no_std]

#[cfg(test)]
extern crate kernel;
#[cfg(test)]
extern crate ktest;

mod address_space;
pub mod arch;
mod error;
mod flush;
pub mod frame_alloc;

use core::alloc::{Layout, LayoutError};
use core::fmt;
use core::fmt::Formatter;
use core::ops::Range;

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

impl fmt::Display for Flags {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        bitflags::parser::to_writer(self, f)
    }
}

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VirtualAddress(usize);
impl VirtualAddress {
    pub const MAX: Self = Self(usize::MAX);

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
        assert!(!overflow, "virtual address underflow");
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

    #[inline]
    pub const fn is_user_accessible(&self) -> bool {
        // This address refers to userspace if it is in the lower half of the
        // canonical addresses.  IOW - if all of the bits in the canonical address
        // mask are zero.
        (self.0 & arch::CANONICAL_ADDRESS_MASK) == 0
    }
}

impl fmt::Display for VirtualAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(format_args!("{:#016x}", self.0))
    }
}

impl fmt::Debug for VirtualAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VirtualAddress")
            .field(&format_args!("{:#016x}", self.0))
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
        f.write_fmt(format_args!("{:#016x}", self.0))
    }
}

impl fmt::Debug for PhysicalAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PhysicalAddress")
            .field(&format_args!("{:#016x}", self.0))
            .finish()
    }
}
pub trait AddressRangeExt {
    fn size(&self) -> usize;
    #[must_use]
    fn add(self, offset: usize) -> Self;
    #[must_use]
    fn as_ptr_range(&self) -> Range<*const u8>;
    #[must_use]
    fn as_mut_ptr_range(&self) -> Range<*mut u8>;
    #[must_use]
    fn align_in(self, align: usize) -> Self;
    #[must_use]
    fn align_out(self, align: usize) -> Self;
    fn align(&self) -> usize;
    fn into_layout(self) -> core::result::Result<Layout, LayoutError>;
    fn is_user_accessible(&self) -> bool;
}

impl AddressRangeExt for Range<PhysicalAddress> {
    fn size(&self) -> usize {
        self.end.sub_addr(self.start)
    }
    fn add(self, offset: usize) -> Self {
        self.start.add(offset)..self.end.add(offset)
    }
    fn as_ptr_range(&self) -> Range<*const u8> {
        self.start.as_raw() as *const u8..self.end.as_raw() as *const u8
    }
    fn as_mut_ptr_range(&self) -> Range<*mut u8> {
        self.start.as_raw() as *mut u8..self.end.as_raw() as *mut u8
    }
    fn align_in(self, align: usize) -> Self {
        self.start.align_up(align)..self.end.align_down(align)
    }
    fn align_out(self, align: usize) -> Self {
        self.start.align_down(align)..self.end.align_up(align)
    }
    // TODO test
    fn align(&self) -> usize {
        self.start.as_raw() & (!self.start.as_raw() + 1)
    }
    fn into_layout(self) -> core::result::Result<Layout, LayoutError> {
        Layout::from_size_align(self.size(), self.align())
    }
    fn is_user_accessible(&self) -> bool {
        unimplemented!("PhysicalAddress is never user accessible")
    }
}

impl AddressRangeExt for Range<VirtualAddress> {
    fn size(&self) -> usize {
        self.end.sub_addr(self.start)
    }
    fn add(self, offset: usize) -> Self {
        self.start.add(offset)..self.end.add(offset)
    }
    fn as_ptr_range(&self) -> Range<*const u8> {
        self.start.as_raw() as *const u8..self.end.as_raw() as *const u8
    }
    fn as_mut_ptr_range(&self) -> Range<*mut u8> {
        self.start.as_raw() as *mut u8..self.end.as_raw() as *mut u8
    }
    fn align_in(self, align: usize) -> Self {
        self.start.align_up(align)..self.end.align_down(align)
    }
    fn align_out(self, align: usize) -> Self {
        self.start.align_down(align)..self.end.align_up(align)
    }
    // TODO test
    fn align(&self) -> usize {
        self.start.as_raw() & (!self.start.as_raw() + 1)
    }
    fn into_layout(self) -> core::result::Result<Layout, LayoutError> {
        Layout::from_size_align(self.size(), self.align())
    }
    fn is_user_accessible(&self) -> bool {
        if self.is_empty() {
            return false;
        }

        self.start.is_user_accessible() && self.end.sub(1).is_user_accessible()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    // #[must_use]
    // #[inline]
    // pub const fn from_phys(phys: PhysicalAddress, physmap_offset: VirtualAddress) -> Self {
    //     physmap_offset.add(phys.as_raw())
    // }
    //
    // #[must_use]
    // #[inline]
    // #[allow(clippy::cast_sign_loss)]
    // pub const fn offset(self, offset: isize) -> Self {
    //     if offset.is_negative() {
    //         self.sub(offset.wrapping_abs() as usize)
    //     } else {
    //         self.add(offset as usize)
    //     }
    // }


    // #[inline]
    // pub const fn is_user_accessible(&self) -> bool {
    //     // This address refers to userspace if it is in the lower half of the
    //     // canonical addresses.  IOW - if all of the bits in the canonical address
    //     // mask are zero.
    //     (self.0 & arch::CANONICAL_ADDRESS_MASK) == 0
    // }

    #[ktest::test]
    fn virt_addr_new() {
        let addr = crate::VirtualAddress::new(0x1234);
        assert_eq!(addr.0, 0x1234);

        let res = panic_unwind::catch_unwind(|| crate::VirtualAddress::new(0));
        assert!(res.is_err());
    }

    proptest! {
        #[ktest::test]
        fn virt_addr_add(a in 0..10000usize, b in 0..10000usize) {
            let addr = crate::VirtualAddress::new(a);
            let addr = addr.add(b);
            assert_eq!(addr.0, a + b);
        }

        #[ktest::test]
        fn virt_addr_sub(a in 0..10000usize, b in 0..10000usize) {
            let addr = crate::VirtualAddress::new(a);
            let addr = addr.sub(b);
            assert_eq!(addr.0, a - b);
        }

        #[ktest::test]
        fn virt_addr_sub_addr(a in 0..10000usize, b in 0..10000usize) {
            let addra = crate::VirtualAddress::new(a);
            let addrb = crate::VirtualAddress::new(b);
            assert_eq!(addra.sub_addr(addrb), a - b);
        }

        #[ktest::test]
        fn virt_addr_is_aligned(a: usize, align in prop::sample::select(&[1, 2, 8, 16, 32, 64, 4096])) {
            let addr = crate::VirtualAddress::new(a);
            assert_eq!(addr.is_aligned(align), a % align == 0);
        }

        #[ktest::test]
        fn virt_addr_align_down(a: usize, align in prop::sample::select(&[1, 2, 8, 16, 32, 64, 4096])) {
            let addr = crate::VirtualAddress::new(a);
            let aligned = addr.align_down(align);
            assert!(aligned.is_aligned(align));
            assert!(aligned <= addr);
        }

        #[ktest::test]
        fn virt_addr_align_up(a: usize, align in prop::sample::select(&[1, 2, 8, 16, 32, 64, 4096])) {
            let addr = crate::VirtualAddress::new(a);
            let aligned = addr.align_up(align);
            assert!(aligned.is_aligned(align));
            assert!(aligned >= addr);
        }
    }

    // proptest! {
    //     #[ktest::test]
    //     fn phys_addr_add(a in 0..10000usize, b in 0..10000usize) {
    //         let addr = crate::PhysicalAddress::new(a);
    //         let addr = addr.add(b);
    //         assert_eq!(addr.0, a + b);
    //     }
    //
    //     #[ktest::test]
    //     fn phys_addr_sub(a in 0..10000usize, b in 0..10000usize) {
    //         let addr = crate::PhysicalAddress::new(a);
    //         let addr = addr.sub(b);
    //         assert_eq!(addr.0, a - b);
    //     }
    //
    //     #[ktest::test]
    //     fn phys_addr_sub_addr(a in 0..10000usize, b in 0..10000usize) {
    //         let addra = crate::PhysicalAddress::new(a);
    //         let addrb = crate::PhysicalAddress::new(b);
    //         assert_eq!(addra.sub_addr(addrb), a - b);
    //     }
    //
    //     #[ktest::test]
    //     fn phys_addr_is_aligned(a: usize, align in prop::sample::select(&[1, 2, 8, 16, 32, 64, 4096])) {
    //         let addr = crate::PhysicalAddress::new(a);
    //         assert_eq!(addr.is_aligned(align), a % align == 0);
    //     }
    //
    //     #[ktest::test]
    //     fn phys_addr_align_down(a: usize, align in prop::sample::select(&[1, 2, 8, 16, 32, 64, 4096])) {
    //         let addr = crate::PhysicalAddress::new(a);
    //         let aligned = addr.align_down(align);
    //         assert!(aligned.is_aligned(align));
    //         assert!(aligned <= addr);
    //     }
    //
    //     #[ktest::test]
    //     fn phys_addr_align_up(a: usize, align in prop::sample::select(&[1, 2, 8, 16, 32, 64, 4096])) {
    //         let addr = crate::PhysicalAddress::new(a);
    //         let aligned = addr.align_up(align);
    //         assert!(aligned.is_aligned(align));
    //         assert!(aligned >= addr);
    //     }
    // }
}
