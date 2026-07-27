// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::{Callsite, IteratorExt, SliceExt};

#[doc(hidden)]
#[must_use]
pub const fn __decide(_site: Callsite) -> bool {
    false
}

#[doc(hidden)]
pub const fn __delay(_site: Callsite) {}

#[doc(hidden)]
#[must_use]
pub const fn __random(_site: Callsite) -> u64 {
    0
}

#[doc(hidden)]
#[inline(always)]
pub fn __assert_stable<T, F>(_site: Callsite, _f: F)
where
    T: PartialEq + core::fmt::Debug,
    F: Fn() -> T,
{
}

impl<T> SliceExt for [T] {
    #[inline(always)]
    fn shuffle(&mut self, _site: Callsite) {}
}

impl<I: Iterator> IteratorExt for I {
    type Shuffled = I;

    #[inline(always)]
    fn shuffled(self, _site: Callsite) -> I {
        self
    }
}
