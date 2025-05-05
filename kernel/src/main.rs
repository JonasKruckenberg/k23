#![no_std]
#![no_main]
#![cfg(target_os = "none")]
#![feature(used_with_arg)]
#![feature(new_range_api)]
#![feature(debug_closure_helpers)]
#![feature(array_chunks)]
#![feature(iter_next_chunk)]
#![feature(if_let_guard)]
#![feature(step_trait)]

extern crate alloc;
extern crate panic_unwind;

mod allocator;
mod arch;
mod backtrace;
mod bootargs;
mod constants;
mod device_tree;
mod irq;
mod mem;
mod start;

use loader_api::BootInfo;

use anyhow::Result;

#[inline(never)]
fn main(cpuid: usize, boot_info: &'static BootInfo, boot_ticks: u64) {
    todo!()
}
