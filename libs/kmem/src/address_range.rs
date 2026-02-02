// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::{PhysicalAddress, VirtualAddress};

pub trait AddressRangeExt {
    type Address;
    fn from_start_len(start: Self::Address, len: usize) -> Self;

    /// Returns `true` if the range contains no addresses.
    fn is_empty(&self) -> bool;

    /// Returns the length of the address range, in bytes.
    fn len(&self) -> usize;

    /// Returns `true` if `address` is contained in the range.
    fn contains(&self, address: &Self::Address) -> bool;

    /// Returns `true` if there exists an address present in both ranges.
    fn overlaps(&self, other: &Self) -> bool;

    /// Returns the intersection of `self` and `other`.
    fn intersect(self, other: Self) -> Self;

    fn align_in(self, align: usize) -> Self
    where
        Self: Sized;
    fn align_out(self, align: usize) -> Self
    where
        Self: Sized;
}

macro_rules! impl_address_range {
    ($address_ty:ident) => {
        impl AddressRangeExt for ::core::ops::Range<$address_ty> {
            type Address = $address_ty;

            fn from_start_len(start: Self::Address, len: usize) -> Self {
                let end = start.add(len);

                Self { start, end }
            }

            fn is_empty(&self) -> bool {
                self.start >= self.end
            }

            fn len(&self) -> usize {
                self.end.offset_from_unsigned(self.start)
            }

            fn contains(&self, address: &Self::Address) -> bool {
                <Self as ::core::ops::RangeBounds<$address_ty>>::contains(self, address)
            }

            fn overlaps(&self, other: &Self) -> bool {
                self.start < other.end && other.start < self.end
            }

            fn intersect(self, other: Self) -> Self {
                Self {
                    start: core::cmp::max(self.start, other.start),
                    end: core::cmp::min(self.end, other.end),
                }
            }

            fn align_in(self, align: usize) -> Self
            where
                Self: Sized,
            {
                self.start.align_up(align)..self.end.align_down(align)
            }

            fn align_out(self, align: usize) -> Self
            where
                Self: Sized,
            {
                self.start.align_down(align)..self.end.align_up(align)
            }
        }
    };
}

impl_address_range!(VirtualAddress);
impl_address_range!(PhysicalAddress);

#[cfg(test)]
mod test {
    use core::ops::Range;

    use super::{AddressRangeExt, *};

    proptest::proptest! {
        #[test]
        fn len(len: usize) {
            let r: Range<VirtualAddress> = Range::from_start_len(VirtualAddress::new(0), len);

            proptest::prop_assert_eq!(len, AddressRangeExt::len(&r));
        }
    }
}
