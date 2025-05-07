// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use core::pin::Pin;
pub use ktest_macros::*;

/// A single test case
pub struct Test {
    pub run: fn() -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
    pub info: TestInfo<'static>,
}

/// Metadata associated with a test case
pub struct TestInfo<'a> {
    pub module: &'a str,
    pub name: &'a str,
    pub ignored: bool,
}
