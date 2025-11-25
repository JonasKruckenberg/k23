// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
use core::fmt::Debug;
use core::num::NonZeroU16;

/// An opaque ID that uniquely identifies an address space relative to all other currently existent
/// address spaces.
///
/// # Notes
///
/// - Address space IDs are unique relative to other *currently existent* address spaces. When an address space
///   gets freed, the same ID may be used for another address space.
/// - Address space IDs are *not* sequential, and do not indicate the order in which
///   Address space are created or any other data.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct Asid(pub(crate) Option<NonZeroU16>);

impl Asid {
    pub const fn global() -> Self {
        Self(None)
    }

    pub const fn new(id: NonZeroU16) -> Self {
        Self(Some(id))
    }

    pub fn is_global(self) -> bool {
        self.0.is_none()
    }
}

impl fmt::Display for Asid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            None => f.write_str("<global>"),
            Some(asid) => write!(f, "<{asid}>"),
        }
    }
}
