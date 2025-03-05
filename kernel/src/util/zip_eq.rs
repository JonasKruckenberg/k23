// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
use core::fmt::Formatter;
use fallible_iterator::FallibleIterator;

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

/// like Iterator::zip but returns an error if one iterator ends before
/// the other. The `param_predicate` is required to select exactly as many
/// elements of `params` as there are elements in `arguments`.
pub struct ZipEq<A, B> {
    a: A,
    b: B,
}

#[derive(Debug)]
pub struct DifferentLengths;

impl fmt::Display for DifferentLengths {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "iterators had different lengths")
    }
}

impl core::error::Error for DifferentLengths {}

impl<A, B> FallibleIterator for ZipEq<A, B>
where
    A: Iterator,
    B: Iterator,
{
    type Item = (A::Item, B::Item);
    type Error = DifferentLengths;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        match (self.a.next(), self.b.next()) {
            (Some(a), Some(b)) => Ok(Some((a, b))),
            (None, None) => Ok(None),
            _ => Err(DifferentLengths),
        }
    }
}
