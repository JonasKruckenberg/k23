// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::sync::atomic::{Ordering, compiler_fence};

use crate::{Callsite, IteratorExt, SliceExt};

#[doc(hidden)]
pub mod sym {
    use crate::Callsite;

    unsafe extern "C" {
        pub safe fn __chaos_decide(site: Callsite) -> bool;
        pub safe fn __chaos_delay(site: Callsite);
        pub safe fn __chaos_random(site: Callsite) -> u64;
    }
}

#[doc(hidden)]
pub fn __decide(site: Callsite) -> bool {
    sym::__chaos_decide(site)
}

#[doc(hidden)]
pub fn __delay(site: Callsite) {
    // `spin_loop` is not a compiler barrier, so without these an observation
    // either side of the window can be merged with the other.
    compiler_fence(Ordering::SeqCst);
    sym::__chaos_delay(site);
    compiler_fence(Ordering::SeqCst);
}

#[doc(hidden)]
pub fn __random(site: Callsite) -> u64 {
    sym::__chaos_random(site)
}

#[doc(hidden)]
pub fn __assert_stable<T, F>(site: Callsite, f: F)
where
    T: PartialEq + core::fmt::Debug,
    F: Fn() -> T,
{
    let before = f();
    __delay(site);
    assert!(
        before == f(),
        "chaos::assert_stable: {before:?} changed across the window",
    );
}

/// A uniform index in `0..n`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the high half of the product is < n, so it is a valid index"
)]
fn pick(site: Callsite, n: usize) -> usize {
    // <https://lemire.me/blog/2016/06/27/a-fast-alternative-to-the-modulo-reduction/>
    ((u128::from(sym::__chaos_random(site)) * n as u128) >> 64_u32) as usize
}

impl<T> SliceExt for [T] {
    fn shuffle(&mut self, site: Callsite) {
        if !sym::__chaos_decide(site) {
            return;
        }
        for i in (1..self.len()).rev() {
            self.swap(i, pick(site, i + 1));
        }
    }
}

/// Iterator returned by [`IteratorExt::shuffled`].
#[doc(hidden)]
pub struct Shuffled<I: Iterator> {
    site: Callsite,
    on: bool,
    iter: I,
    buf: arrayvec::ArrayVec<I::Item, 16>,
}

impl<I: Iterator> IteratorExt for I {
    type Shuffled = Shuffled<I>;

    fn shuffled(self, site: Callsite) -> Shuffled<I> {
        Shuffled {
            site,
            on: sym::__chaos_decide(site),
            iter: self,
            buf: arrayvec::ArrayVec::new(),
        }
    }
}

impl<I: Iterator> Iterator for Shuffled<I> {
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.on {
            return self.iter.next();
        }
        if self.buf.is_empty() {
            while self.buf.len() < self.buf.capacity() {
                let Some(item) = self.iter.next() else { break };
                self.buf.push(item);
            }
            if self.buf.is_empty() {
                return None;
            }
        }
        Some(self.buf.swap_remove(pick(self.site, self.buf.len())))
    }
}
