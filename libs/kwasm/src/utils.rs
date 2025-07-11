/// Helper macro to generate accessors for an enum.
macro_rules! enum_accessors {
    (@$bind:ident, $variant:ident, $ty:ty, $is:ident, $get:ident, $unwrap:ident, $cvt:expr) => {
        ///  Returns true when the enum is the correct variant.
        pub fn $is(&self) -> bool {
            matches!(self, Self::$variant(_))
        }

        ///  Returns the variant's value, returning None if it is not the correct type.
        #[inline]
        pub fn $get(&self) -> Option<$ty> {
            if let Self::$variant($bind) = self {
                Some($cvt)
            } else {
                None
            }
        }

        /// Returns the variant's value, panicking if it is not the correct type.
        ///
        /// # Panics
        ///
        /// Panics if `self` is not of the right type.
        #[inline]
        pub fn $unwrap(&self) -> $ty {
            self.$get().expect(concat!("expected ", stringify!($ty)))
        }
    };
    ($bind:ident $(($variant:ident($ty:ty) $is:ident $get:ident $unwrap:ident $cvt:expr))*) => ($(enum_accessors!{@$bind, $variant, $ty, $is, $get, $unwrap, $cvt})*)
}

/// Like `enum_accessors!`, but generated methods take ownership of `self`.
macro_rules! owned_enum_accessors {
    ($bind:ident $(($variant:ident($ty:ty) $get:ident $cvt:expr))*) => ($(
        /// Attempt to access the underlying value of this `Val`, returning
        /// `None` if it is not the correct type.
        #[inline]
        pub fn $get(self) -> Option<$ty> {
            if let Self::$variant($bind) = self {
                Some($cvt)
            } else {
                None
            }
        }
    )*)
}

/// Like `offset_of!`, but returns a `u32`.
///
/// # Panics
///
/// Panics if the offset is too large to fit in a `u32`.
macro_rules! u32_offset_of {
    ($ty:ident, $field:ident) => {
        u32::try_from(core::mem::offset_of!($ty, $field)).unwrap()
    };
}

use core::any::type_name;
use core::cmp;
pub(crate) use {enum_accessors, owned_enum_accessors, u32_offset_of};

/// Like `mem::size_of` but returns `u8` instead of `usize`
/// # Panics
///
/// Panics if the size of `T` is greater than `u8::MAX`.
pub fn u8_size_of<T: Sized>() -> u8 {
    u8::try_from(size_of::<T>()).expect("type size is too large to be represented as a u8")
}

pub trait IteratorExt {
    fn zip_eq<U>(self, other: U) -> ZipEq<Self, <U as IntoIterator>::IntoIter>
    where
        Self: Sized,
        U: IntoIterator;
}

impl<I> IteratorExt for I
where
    I: Iterator,
{
    fn zip_eq<U>(self, other: U) -> ZipEq<Self, <U as IntoIterator>::IntoIter>
    where
        Self: Sized,
        U: IntoIterator,
    {
        ZipEq {
            a: self,
            b: other.into_iter(),
        }
    }
}

/// like Iterator::zip but panics if one iterator ends before
/// the other. The `param_predicate` is required to select exactly as many
/// elements of `params` as there are elements in `arguments`.
pub struct ZipEq<A, B> {
    a: A,
    b: B,
}

impl<A, B> Iterator for ZipEq<A, B>
where
    A: Iterator,
    B: Iterator,
{
    type Item = (A::Item, B::Item);

    fn next(&mut self) -> Option<Self::Item> {
        match (self.a.next(), self.b.next()) {
            (Some(a), Some(b)) => Some((a, b)),
            (None, None) => None,
            (None, _) => panic!(
                "iterators had different lengths. {} was shorter than {}",
                type_name::<A>(),
                type_name::<B>()
            ),
            (_, None) => panic!(
                "iterators had different lengths. {} was shorter than {}",
                type_name::<B>(),
                type_name::<A>()
            ),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let (a_min, a_max) = self.a.size_hint();
        let (b_min, b_max) = self.a.size_hint();
        (
            cmp::min(a_min, b_min),
            a_max
                .and_then(|a| Some((a, b_max?)))
                .map(|(a, b)| cmp::min(a, b)),
        )
    }
}

impl<A, B> ExactSizeIterator for ZipEq<A, B>
where
    A: ExactSizeIterator,
    B: ExactSizeIterator,
{
    fn len(&self) -> usize {
        debug_assert_eq!(self.a.len(), self.b.len());
        self.a.len()
    }
}
