// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::task::Waker;

use k23_arrayvec::ArrayVec;

const NUM_WAKERS: usize = 32;

pub struct WakeBatch {
    inner: ArrayVec<Waker, NUM_WAKERS>,
}

impl Default for WakeBatch {
    fn default() -> Self {
        Self::new()
    }
}

impl WakeBatch {
    pub const fn new() -> Self {
        Self {
            inner: ArrayVec::new(),
        }
    }

    /// Adds a [`Waker`] to the batch, returning `true` if the batch needs to be flushed because it
    /// is full.
    pub fn add_waker(&mut self, waker: Waker) -> bool {
        self.inner.push(waker);
        self.inner.is_full()
    }

    pub fn wake_all(&mut self) {
        for waker in self.inner.drain(..) {
            waker.wake();
        }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}
