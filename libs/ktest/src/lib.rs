#![no_std]

pub use ktest_macros::*;
use loader_api::BootInfo;

/// A single test case
pub struct Test {
    pub run: fn(&'static BootInfo),
    pub info: TestInfo<'static>,
}

/// Metadata associated with a test case
pub struct TestInfo<'a> {
    pub module: &'a str,
    pub name: &'a str,
    pub ignored: bool,
}
