// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod paged;
mod wired;

pub use paged::PagedVmo;
use sync::RwLock;
pub use wired::WiredVmo;

#[derive(Debug)]
pub enum Vmo {
    Wired(WiredVmo),
    Paged(RwLock<PagedVmo>),
}

impl Vmo {

    pub fn is_valid_offset(&self, offset: usize) -> bool {
        match self {
            Vmo::Wired(vmo) => vmo.is_valid_offset(offset),
            Vmo::Paged(vmo) => vmo.read().is_valid_offset(offset)
        }
    }
}