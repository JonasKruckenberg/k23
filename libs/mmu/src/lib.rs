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

macro_rules! address_impl {
    ($addr:ident) => {
        impl $addr {
            pub const MAX: Self = Self(usize::MAX);
            pub const MIN: Self = Self(0);
            pub const BITS: u32 = usize::BITS;

            #[must_use]
            #[inline]
            pub const fn checked_add(self, rhs: usize) -> Option<Self> {
                if let Some(out) = self.0.checked_add(rhs) {
                    Some(Self(out))
                } else {
                    None
                }
            }

            #[must_use]
            #[inline]
            pub const fn checked_add_signed(self, rhs: isize) -> Option<Self> {
                if let Some(out) = self.0.checked_add_signed(rhs) {
                    Some(Self(out))
                } else {
                    None
                }
            }

            #[must_use]
            #[inline]
            pub const fn checked_sub(self, rhs: usize) -> Option<Self> {
                if let Some(out) = self.0.checked_sub(rhs) {
                    Some(Self(out))
                } else {
                    None
                }
            }
            #[must_use]
            #[inline]
            pub const fn checked_div(self, rhs: usize) -> Option<Self> {
                if let Some(out) = self.0.checked_div(rhs) {
                    Some(Self(out))
                } else {
                    None
                }
            }
            #[must_use]
            #[inline]
            pub const fn checked_mul(self, rhs: usize) -> Option<Self> {
                if let Some(out) = self.0.checked_mul(rhs) {
                    Some(Self(out))
                } else {
                    None
                }
            }
            #[must_use]
            #[inline]
            pub const fn checked_shl(self, rhs: u32) -> Option<Self> {
                if let Some(out) = self.0.checked_shl(rhs) {
                    Some(Self(out))
                } else {
                    None
                }
            }
            #[must_use]
            #[inline]
            pub const fn checked_shr(self, rhs: u32) -> Option<Self> {
                if let Some(out) = self.0.checked_shr(rhs) {
                    Some(Self(out))
                } else {
                    None
                }
            }
            // #[must_use]
            // #[inline]
            // pub const fn saturating_add(self, rhs: usize) -> Self {
            //     Self(self.0.saturating_add(rhs))
            // }
            // #[must_use]
            // #[inline]
            // pub const fn saturating_add_signed(self, rhs: isize) -> Self {
            //     Self(self.0.saturating_add_signed(rhs))
            // }
            // #[must_use]
            // #[inline]
            // pub const fn saturating_div(self, rhs: usize) -> Self {
            //     Self(self.0.saturating_div(rhs))
            // }
            // #[must_use]
            // #[inline]
            // pub const fn saturating_sub(self, rhs: usize) -> Self {
            //     Self(self.0.saturating_sub(rhs))
            // }
            // #[must_use]
            // #[inline]
            // pub const fn saturating_mul(self, rhs: usize) -> Self {
            //     Self(self.0.saturating_mul(rhs))
            // }
            #[must_use]
            #[inline]
            pub const fn overflowing_shl(self, rhs: u32) -> (Self, bool) {
                let (a, b) = self.0.overflowing_shl(rhs);
                (Self(a), b)
            }
            #[must_use]
            #[inline]
            pub const fn overflowing_shr(self, rhs: u32) -> (Self, bool) {
                let (a, b) = self.0.overflowing_shr(rhs);
                (Self(a), b)
            }

            #[must_use]
            #[inline]
            pub const fn checked_sub_addr(self, rhs: Self) -> Option<usize> {
                self.0.checked_sub(rhs.0)
            }

            // #[must_use]
            // #[inline]
            // pub const fn saturating_sub_addr(self, rhs: Self) -> usize {
            //     self.0.saturating_sub(rhs.0)
            // }

            #[must_use]
            #[inline]
            pub const fn is_aligned_to(&self, align: usize) -> bool {
                assert!(
                    align.is_power_of_two(),
                    "is_aligned_to: align is not a power-of-two"
                );

                self.0 & (align - 1) == 0
            }

            #[must_use]
            #[inline]
            pub const fn checked_align_up(self, align: usize) -> Option<Self> {
                if !align.is_power_of_two() {
                    panic!("checked_align_up: align is not a power-of-two");
                }

                // SAFETY: `align` has been checked to be a power of 2 above
                let align_minus_one = unsafe { align.unchecked_sub(1) };

                // addr.wrapping_add(align_minus_one) & 0usize.wrapping_sub(align)
                if let Some(addr_plus_align) = self.0.checked_add(align_minus_one) {
                    let aligned = Self(addr_plus_align & 0usize.wrapping_sub(align));
                    debug_assert!(aligned.is_aligned_to(align));
                    debug_assert!(aligned.0 >= self.0);
                    Some(aligned)
                } else {
                    None
                }
            }

            // #[must_use]
            // #[inline]
            // pub const fn wrapping_align_up(self, align: usize) -> Self {
            //     if !align.is_power_of_two() {
            //         panic!("checked_align_up: align is not a power-of-two");
            //     }
            //
            //     // SAFETY: `align` has been checked to be a power of 2 above
            //     let align_minus_one = unsafe { align.unchecked_sub(1) };
            //
            //     // addr.wrapping_add(align_minus_one) & 0usize.wrapping_sub(align)
            //     let out = addr.wrapping_add(align_minus_one) & 0usize.wrapping_sub(align);
            //     debug_assert!(out.is_aligned_to(align));
            //     out
            // }

            #[must_use]
            #[inline]
            pub const fn align_down(self, align: usize) -> Self {
                if !align.is_power_of_two() {
                    panic!("checked_align_up: align is not a power-of-two");
                }

                let aligned = Self(self.0 & 0usize.wrapping_sub(align));
                debug_assert!(aligned.is_aligned_to(align));
                debug_assert!(aligned.0 <= self.0);
                aligned
            }

            #[inline]
            pub const fn as_ptr(self) -> *const u8 {
                self.0 as *const u8
            }
            #[inline]
            pub const fn as_mut_ptr(self) -> *mut u8 {
                self.0 as *mut u8
            }
            #[inline]
            pub const fn get(self) -> usize {
                self.0
            }
        }

        impl fmt::Display for $addr {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_fmt(format_args!("{:#016x}", self.0))
            }
        }

        impl fmt::Debug for $addr {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple(stringify!($addr))
                    .field(&format_args!("{:#016x}", self.0))
                    .finish()
            }
        }
        impl From<usize> for $addr {
            fn from(value: usize) -> Self {
                $addr(value)
            }
        }
    };
}

macro_rules! address_range_impl {
    () => {
        fn size(&self) -> usize {
            debug_assert!(self.start <= self.end);
            let is = self.end.checked_sub_addr(self.start).unwrap_or_default();
            let should = if self.is_empty() {
                0
            } else {
                self.end.get() - self.start.get()
            };
            debug_assert_eq!(is, should);
            is
        }
        fn checked_add(self, offset: usize) -> Option<Self> {
            Some(self.start.checked_add(offset)?..self.end.checked_add(offset)?)
        }
        fn as_ptr_range(&self) -> Range<*const u8> {
            self.start.as_ptr()..self.end.as_ptr()
        }
        fn as_mut_ptr_range(&self) -> Range<*mut u8> {
            self.start.as_mut_ptr()..self.end.as_mut_ptr()
        }
        fn checked_align_in(self, align: usize) -> Option<Self>
        where
            Self: Sized,
        {
            let res = self.start.checked_align_up(align)?..self.end.align_down(align);
            Some(res)
        }
        fn checked_align_out(self, align: usize) -> Option<Self>
        where
            Self: Sized,
        {
            let res = self.start.align_down(align)..self.end.checked_align_up(align)?;
            // aligning outwards can only increase the size
            debug_assert!(res.start.0 <= res.end.0);
            Some(res)
        }
        // fn saturating_align_in(self, align: usize) -> Self {
        //     self.start.saturating_align_up(align)..self.end.saturating_align_down(align)
        // }
        // fn saturating_align_out(self, align: usize) -> Self {
        //     self.start.saturating_align_down(align)..self.end.saturating_align_up(align)
        // }

        // TODO test
        fn align(&self) -> usize {
            self.start.0 & (!self.start.0 + 1)
        }
        fn into_layout(self) -> core::result::Result<Layout, LayoutError> {
            Layout::from_size_align(self.size(), self.align())
        }
    };
}

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VirtualAddress(usize);
address_impl!(VirtualAddress);

impl VirtualAddress {
    #[must_use]
    pub const fn new(n: usize) -> Option<Self> {
        if (n & arch::CANONICAL_ADDRESS_MASK).wrapping_sub(1) >= arch::CANONICAL_ADDRESS_MASK - 1 {
            Some(Self(n))
        } else {
            None
        }
    }
    #[must_use]
    pub fn from_phys(phys: PhysicalAddress, phys_offset: VirtualAddress) -> Option<VirtualAddress> {
        phys_offset.checked_add(phys.0)
    }

    #[inline]
    pub const fn is_user_accessible(&self) -> bool {
        // This address refers to userspace if it is in the lower half of the
        // canonical addresses.  IOW - if all of the bits in the canonical address
        // mask are zero.
        (self.0 & arch::CANONICAL_ADDRESS_MASK) == 0
    }
}

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PhysicalAddress(usize);
address_impl!(PhysicalAddress);
impl PhysicalAddress {
    #[must_use]
    pub const fn new(n: usize) -> Self {
        Self(n)
    }
}

pub trait AddressRangeExt {
    fn size(&self) -> usize;
    #[must_use]
    fn checked_add(self, offset: usize) -> Option<Self>
    where
        Self: Sized;
    #[must_use]
    fn as_ptr_range(&self) -> Range<*const u8>;
    #[must_use]
    fn as_mut_ptr_range(&self) -> Range<*mut u8>;
    #[must_use]
    fn checked_align_in(self, align: usize) -> Option<Self>
    where
        Self: Sized;
    #[must_use]
    fn checked_align_out(self, align: usize) -> Option<Self>
    where
        Self: Sized;
    // #[must_use]
    // fn saturating_align_in(self, align: usize) -> Self;
    // #[must_use]
    // fn saturating_align_out(self, align: usize) -> Self;
    fn align(&self) -> usize;
    fn into_layout(self) -> core::result::Result<Layout, LayoutError>;
    fn is_user_accessible(&self) -> bool;
}

impl AddressRangeExt for Range<PhysicalAddress> {
    address_range_impl!();
    fn is_user_accessible(&self) -> bool {
        unimplemented!("PhysicalAddress is never user accessible")
    }
}

impl AddressRangeExt for Range<VirtualAddress> {
    address_range_impl!();

    fn is_user_accessible(&self) -> bool {
        if self.is_empty() {
            return false;
        }
        let Some(end_minus_one) = self.end.checked_sub(1) else {
            return false;
        };

        self.start.is_user_accessible() && end_minus_one.is_user_accessible()
    }
}

static_assertions::const_assert!(VirtualAddress(0xffffffc000000000).is_aligned_to(4096));
static_assertions::const_assert_eq!(VirtualAddress(0xffffffc0000156e8).align_down(4096).0, 0xffffffc000015000);
static_assertions::const_assert_eq!(VirtualAddress(0xffffffc000000000).checked_align_up(4096).unwrap().0, 0xffffffc000000000);
static_assertions::const_assert_eq!(VirtualAddress(0xffffffc0000156e8).checked_align_up(4096).unwrap().0, 0xffffffc000016000);

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(VirtualAddress::new(0x1234).unwrap().0, 0x1234);
        assert_eq!(VirtualAddress::new(0x0).unwrap().0, 0x0);

        assert!(VirtualAddress::new(0x0000004000000000).is_none());
        assert!(VirtualAddress::new(0xffffffbfffffffff).is_none());

        assert_eq!(
            VirtualAddress::new(0xffffffc000000000).unwrap().0,
            0xffffffc000000000
        );
        assert_eq!(VirtualAddress::new(usize::MAX).unwrap().0, usize::MAX);
    }

    proptest! {
        #[ktest::test]
        fn virt_addr_add(a in 0..10000usize, b in 0..10000usize) {
            let addr = VirtualAddress::new(a).unwrap();
            assert_eq!(addr.checked_add(b), a.checked_add(b).map(VirtualAddress));
        }

        #[ktest::test]
        fn virt_addr_sub(a in 0..10000usize, b in 0..10000usize) {
            let addr = VirtualAddress::new(a).unwrap();
            assert_eq!(addr.checked_sub(b), a.checked_sub(b).map(VirtualAddress));
        }

        #[ktest::test]
        fn virt_addr_sub_addr(a in 0..10000usize, b in 0..10000usize) {
            let addra = VirtualAddress::new(a).unwrap();
            let addrb = VirtualAddress::new(b).unwrap();
            assert_eq!(addra.checked_sub_addr(addrb), a.checked_sub(b));
        }

        #[ktest::test]
        fn virt_addr_is_aligned(a: usize, align in prop::sample::select(&[1, 2, 8, 16, 32, 64, 4096])) {
            let addr = VirtualAddress::new(a).unwrap();
            assert_eq!(addr.is_aligned_to(align), a % align == 0);
        }

        #[ktest::test]
        fn virt_addr_align_down(a: usize, align in prop::sample::select(&[1, 2, 8, 16, 32, 64, 4096])) {
            let addr = VirtualAddress::new(a).unwrap();
            let aligned = addr.checked_align_down(align).unwrap();
            assert!(aligned.is_aligned_to(align));
            assert!(aligned <= addr);
        }

        #[ktest::test]
        fn virt_addr_align_up(a: usize, align in prop::sample::select(&[1, 2, 8, 16, 32, 64, 4096])) {
            let addr = VirtualAddress::new(a).unwrap();
            let aligned = addr.checked_align_up(align).unwrap();
            assert!(aligned.is_aligned_to(align));
            assert!(aligned >= addr);
        }

        #[ktest::test]
        fn virt_addr_is_user_accessible(a in 0..0x0000004000000000usize) {
            let addr = VirtualAddress::new(a).unwrap();
            assert!(addr.is_user_accessible());
        }
    }

    proptest! {
        #[ktest::test]
        fn phys_addr_add(a in 0..10000usize, b in 0..10000usize) {
            let addr = PhysicalAddress::new(a);
            assert_eq!(addr.checked_add(b), a.checked_add(b).map(PhysicalAddress));
        }

        #[ktest::test]
        fn phys_addr_sub(a in 0..10000usize, b in 0..10000usize) {
            let addr = PhysicalAddress::new(a);
            assert_eq!(addr.checked_sub(b), a.checked_sub(b).map(PhysicalAddress));
        }

        #[ktest::test]
        fn phys_addr_sub_addr(a in 0..10000usize, b in 0..10000usize) {
            let addra = PhysicalAddress::new(a);
            let addrb = PhysicalAddress::new(b);
            assert_eq!(addra.checked_sub_addr(addrb), a.checked_sub(b));
        }

        #[ktest::test]
        fn phys_addr_is_aligned(a: usize, align in prop::sample::select(&[1, 2, 8, 16, 32, 64, 4096])) {
            let addr = PhysicalAddress::new(a);
            assert_eq!(addr.is_aligned_to(align), a % align == 0);
        }

        #[ktest::test]
        fn phys_addr_align_down(a: usize, align in prop::sample::select(&[1, 2, 8, 16, 32, 64, 4096])) {
            let addr = PhysicalAddress::new(a);
            let aligned = addr.checked_align_down(align).unwrap();
            assert!(aligned.is_aligned_to(align));
            assert!(aligned <= addr);
        }

        #[ktest::test]
        fn phys_addr_align_up(a: usize, align in prop::sample::select(&[1, 2, 8, 16, 32, 64, 4096])) {
            let addr = PhysicalAddress::new(a);
            let aligned = addr.checked_align_up(align).unwrap();
            assert!(aligned.is_aligned_to(align));
            assert!(aligned >= addr);
        }
    }
}
