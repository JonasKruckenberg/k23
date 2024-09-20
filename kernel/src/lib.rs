#![no_std]
#![no_main]
#![allow(internal_features)]
#![feature(used_with_arg, naked_functions, thread_local, allocator_api)]
#![feature(panic_can_unwind, std_internals, fmt_internals)]

extern crate alloc;

pub mod allocator;
pub mod arch;
mod frame_alloc;
pub mod kconfig;
pub mod runtime;
mod start;

#[cfg(test)]
mod tests {
    #[ktest::test]
    fn feature() {
        assert!(false);
    }
}