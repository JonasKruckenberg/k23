// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![cfg_attr(not(test), no_std)]
#![feature(thread_local)]

extern crate alloc;

mod cell;
mod domain;
mod head;
mod loom;
mod reader;
#[cfg(test)]
mod tests;

pub use cell::{QsbrCell, Shared};
pub use domain::QsbrDomain;
pub use head::QsbrHead;
pub use reader::{Guard, QsbrReader};
