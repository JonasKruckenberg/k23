// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::any::type_name;
use core::cmp;

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
