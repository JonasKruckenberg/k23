// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

pub mod oneshot;
pub mod wait_cell;

pub use wait_cell::WaitCell;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Closed(());

impl Closed {
    pub(crate) const fn new() -> Self {
        Self(())
    }
}

impl core::fmt::Display for Closed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.pad("closed")
    }
}

impl core::error::Error for Closed {}
