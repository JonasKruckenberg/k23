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
