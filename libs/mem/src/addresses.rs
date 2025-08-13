// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::{Layout, LayoutError};
use core::ops::Range;

use crate::address_space::RawAddressSpace;

macro_rules! impl_address {
    ($address_ty:ident) => {
        impl $address_ty {
            pub const MAX: Self = Self(usize::MAX);
            pub const MIN: Self = Self(0);
            pub const ZERO: Self = Self(0);
            pub const BITS: u32 = usize::BITS;

            #[inline]
            pub const fn get(&self) -> usize {
                self.0
            }

            #[must_use]
            #[inline]
            pub fn from_ptr<T: ?Sized>(ptr: *const T) -> Self {
                Self(ptr.expose_provenance())
            }

            #[must_use]
            #[inline]
            pub fn from_mut_ptr<T: ?Sized>(ptr: *mut T) -> Self {
                Self(ptr.expose_provenance())
            }

            #[must_use]
            #[inline]
            pub fn from_non_null<T: ?Sized>(ptr: ::core::ptr::NonNull<T>) -> Self {
                Self(ptr.addr().get())
            }

            #[inline]
            pub fn as_ptr(self) -> *const u8 {
                ::core::ptr::with_exposed_provenance(self.0)
            }

            #[inline]
            pub fn as_mut_ptr(self) -> *mut u8 {
                ::core::ptr::with_exposed_provenance_mut(self.0)
            }

            #[inline]
            pub fn as_non_null(self) -> Option<::core::ptr::NonNull<u8>> {
                ::core::num::NonZeroUsize::new(self.0)
                    .map(::core::ptr::NonNull::with_exposed_provenance)
            }

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

            #[inline]
            pub const fn alignment(&self) -> usize {
                self.0 & (!self.0 + 1)
            }

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
        }

        impl ::core::fmt::Display for $address_ty {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_fmt(format_args!("{:#018x}", self.0)) // 18 digits to account for the leading 0x
            }
        }

        impl ::core::fmt::Debug for $address_ty {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_tuple(stringify!($address_ty))
                    .field(&format_args!("{:#018x}", self.0)) // 18 digits to account for the leading 0x
                    .finish()
            }
        }
    };
}

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VirtualAddress(usize);
impl_address!(VirtualAddress);

impl VirtualAddress {
    #[must_use]
    pub const fn new(n: usize) -> Self {
        Self(n)
    }

    pub const fn is_canonical<A: RawAddressSpace>(self) -> bool {
        (self.0 & A::CANONICAL_ADDRESS_MASK).wrapping_sub(1) >= A::CANONICAL_ADDRESS_MASK - 1
    }

    #[inline]
    pub const fn is_user_accessible<A: RawAddressSpace>(self) -> bool {
        // This address refers to userspace if it is in the lower half of the
        // canonical addresses.  IOW - if all of the bits in the canonical address
        // mask are zero.
        (self.0 & A::CANONICAL_ADDRESS_MASK) == 0
    }
}

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PhysicalAddress(usize);
impl_address!(PhysicalAddress);

impl PhysicalAddress {
    pub const fn new(n: usize) -> Self {
        Self(n)
    }
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
            Some(Range::from(
                self.start.checked_add(offset)?..self.end.checked_add(offset)?,
            ))
        }
        fn as_ptr_range(&self) -> Range<*const u8> {
            Range::from(self.start.as_ptr()..self.end.as_ptr())
        }
        fn as_mut_ptr_range(&self) -> Range<*mut u8> {
            Range::from(self.start.as_mut_ptr()..self.end.as_mut_ptr())
        }
        fn checked_align_in(self, align: usize) -> Option<Self>
        where
            Self: Sized,
        {
            let res = Range::from(self.start.checked_align_up(align)?..self.end.align_down(align));
            Some(res)
        }
        fn checked_align_out(self, align: usize) -> Option<Self>
        where
            Self: Sized,
        {
            let res = Range::from(self.start.align_down(align)..self.end.checked_align_up(align)?);
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
        fn alignment(&self) -> usize {
            self.start.alignment()
        }
        fn into_layout(self) -> core::result::Result<Layout, core::alloc::LayoutError> {
            Layout::from_size_align(self.size(), self.alignment())
        }
        fn is_overlapping(&self, other: &Self) -> bool {
            (self.start < other.end) & (other.start < self.end)
        }
        fn difference(&self, other: Self) -> (Option<Self>, Option<Self>) {
            debug_assert!(self.is_overlapping(&other));
            let a = Range::from(self.start..other.start);
            let b = Range::from(other.end..self.end);
            ((!a.is_empty()).then_some(a), (!b.is_empty()).then_some(b))
        }
        fn clamp(&self, range: Self) -> Self {
            Range::from(self.start.max(range.start)..self.end.min(range.end))
        }
    };
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
    fn alignment(&self) -> usize;
    fn into_layout(self) -> Result<Layout, LayoutError>;
    fn is_overlapping(&self, other: &Self) -> bool;
    fn difference(&self, other: Self) -> (Option<Self>, Option<Self>)
    where
        Self: Sized;
    fn clamp(&self, range: Self) -> Self;
    fn is_user_accessible<A: RawAddressSpace>(&self) -> bool;
}

impl AddressRangeExt for Range<PhysicalAddress> {
    address_range_impl!();
    fn is_user_accessible<A: RawAddressSpace>(&self) -> bool {
        unimplemented!("PhysicalAddress is never user accessible")
    }
}

impl AddressRangeExt for Range<VirtualAddress> {
    address_range_impl!();

    fn is_user_accessible<A: RawAddressSpace>(&self) -> bool {
        if self.is_empty() {
            return false;
        }
        let Some(end_minus_one) = self.end.checked_sub(1) else {
            return false;
        };

        self.start.is_user_accessible::<A>() && end_minus_one.is_user_accessible::<A>()
    }
}
