// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::AllocError;
use core::fmt;

#[derive(Debug)]
pub enum SpawnError {
    Closed,
    Alloc,
}

impl From<AllocError> for SpawnError {
    fn from(_: AllocError) -> Self {
        Self::Alloc
    }
}

impl From<Closed> for SpawnError {
    fn from(_: Closed) -> Self {
        Self::Closed
    }
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpawnError::Closed => f.write_str("executor was closed"),
            SpawnError::Alloc => f.write_str("memory allocation failed"),
        }
    }
}

impl core::error::Error for SpawnError {}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Closed(pub(crate) ());

impl fmt::Display for Closed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("closed")
    }
}

impl core::error::Error for Closed {}
