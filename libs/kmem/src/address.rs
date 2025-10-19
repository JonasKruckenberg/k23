// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

macro_rules! impl_address_from {
    ($address_ty:ident, $int_ty:ident) => {
        impl From<$int_ty> for $address_ty {
            fn from(value: $int_ty) -> Self {
                $address_ty(usize::from(value))
            }
        }
    };
}

macro_rules! impl_address_try_from {
    ($address_ty:ident, $int_ty:ident) => {
        impl TryFrom<$int_ty> for $address_ty {
            type Error = <usize as TryFrom<$int_ty>>::Error;

            fn try_from(value: $int_ty) -> Result<Self, Self::Error> {
                usize::try_from(value).map($address_ty)
            }
        }
    };
}

macro_rules! impl_address {
    ($address_ty:ident) => {
        impl $address_ty {
            pub const MAX: Self = Self(usize::MAX);
            pub const MIN: Self = Self(usize::MIN);

            #[must_use]
            pub const fn new(n: usize) -> Self {
                Self(n)
            }

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

            /// Adds an unsigned offset to this address, panicking if overflow occurred.
            #[must_use]
            #[inline]
            pub const fn add(self, offset: usize) -> Self {
                Self(self.0 + offset)
            }

            /// Subtracts an unsigned offset from this address, panicking if overflow occurred.
            #[must_use]
            #[inline]
            pub const fn sub(self, offset: usize) -> Self {
                Self(self.0 - offset)
            }

            /// Adds a signed offset in bytes to this address, panicking if overflow occurred.
            #[must_use]
            #[inline]
            pub const fn offset(self, offset: isize) -> Self {
                let (a, b) = self.0.overflowing_add_signed(offset);
                if b {
                    panic!("attempt to offset with overflow")
                } else {
                    Self(a)
                }
            }

            /// Adds an unsigned offset to this address, wrapping around at the boundary of the type.
            #[must_use]
            #[inline]
            pub const fn wrapping_add(self, offset: usize) -> Self {
                Self(self.0.wrapping_add(offset))
            }

            /// Subtracts an unsigned offset from this address, wrapping around at the boundary of the type.
            #[must_use]
            #[inline]
            pub const fn wrapping_sub(self, offset: usize) -> Self {
                Self(self.0.wrapping_sub(offset))
            }

            /// Adds a signed offset in bytes to this address, wrapping around at the boundary of the type.
            #[must_use]
            #[inline]
            pub const fn wrapping_offset(self, offset: isize) -> Self {
                Self(self.0.wrapping_add_signed(offset))
            }

            /// Calculates the distance between two addresses in bytes.
            #[must_use]
            #[inline]
            #[expect(clippy::cast_possible_wrap, reason = "intentional wrapping here")]
            pub const fn offset_from(self, origin: Self) -> isize {
                self.0.wrapping_sub(origin.0) as isize
            }

            /// Calculates the distance between two addresses in bytes, _where it’s known that `self`
            /// is equal to or greater than `origin`_.
            ///
            /// # Panics
            ///
            /// Panics if `self` is less than `origin`.
            #[must_use]
            #[inline]
            pub const fn offset_from_unsigned(self, origin: Self) -> usize {
                let (a, b) = self.0.overflowing_sub(origin.0);
                if b {
                    panic!("attempt to subtract with overflow")
                } else {
                    a
                }
            }

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
            pub const fn align_up(self, align: usize) -> Self {
                if !align.is_power_of_two() {
                    panic!("checked_align_up: align is not a power-of-two");
                }

                // SAFETY: `align` has been checked to be a power of 2 above
                let align_minus_one = unsafe { align.unchecked_sub(1) };

                let aligned =
                    Self(self.0.wrapping_add(align_minus_one) & 0usize.wrapping_sub(align));
                debug_assert!(aligned.is_aligned_to(align));
                debug_assert!(aligned.0 >= self.0);
                aligned
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

        impl_address_from!($address_ty, usize);
        impl_address_from!($address_ty, u8);
        impl_address_from!($address_ty, u16);
        impl_address_try_from!($address_ty, i8);
        impl_address_try_from!($address_ty, i16);
        impl_address_try_from!($address_ty, i32);
        impl_address_try_from!($address_ty, i64);
        impl_address_try_from!($address_ty, i128);
        impl_address_try_from!($address_ty, u32);
        impl_address_try_from!($address_ty, u64);
        impl_address_try_from!($address_ty, u128);

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

        impl core::iter::Step for $address_ty {
            fn steps_between(start: &Self, end: &Self) -> (usize, Option<usize>) {
                core::iter::Step::steps_between(&start.0, &end.0)
            }

            fn forward_checked(start: Self, count: usize) -> Option<Self> {
                core::iter::Step::forward_checked(start.0, count).map(Self)
            }

            fn forward(start: Self, count: usize) -> Self {
                Self(core::iter::Step::forward(start.0, count))
            }

            unsafe fn forward_unchecked(start: Self, count: usize) -> Self {
                // Safety: checked by the caller
                Self(unsafe { core::iter::Step::forward_unchecked(start.0, count) })
            }

            fn backward_checked(start: Self, count: usize) -> Option<Self> {
                core::iter::Step::backward_checked(start.0, count).map(Self)
            }

            fn backward(start: Self, count: usize) -> Self {
                Self(core::iter::Step::backward(start.0, count))
            }

            unsafe fn backward_unchecked(start: Self, count: usize) -> Self {
                // Safety: checked by the caller
                Self(unsafe { core::iter::Step::backward_unchecked(start.0, count) })
            }
        }
    };
}

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VirtualAddress(usize);
impl_address!(VirtualAddress);

#[repr(transparent)]
#[derive(Default, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct PhysicalAddress(usize);
impl_address!(PhysicalAddress);

impl PhysicalAddress {
    pub fn to_virt(self, physmap: VirtualAddress) -> VirtualAddress {
        physmap.add(self.0)
    }
}