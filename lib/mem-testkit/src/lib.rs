// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Std-backed test harness for the mem-* crates and their consumers: an emulated
//! [`Machine`] (per-CPU TLB, discontiguous physical memory) with an [`EmulateArch`]
//! `Arch` backend, a deallocating [`TestFrameAllocator`], `proptest` strategies, and
//! the cross-architecture test macros [`for_arch!`] / [`archtest!`].
//!
//! This is a test/dev dependency only; it is never linked into a bare-metal binary.
//! It lives in its own crate (rather than behind a `mem-core`/`mem-mmu` feature) so the
//! libraries stay unconditionally `no_std` and there is exactly one configured build of
//! each across every dependency graph.

#![feature(debug_closure_helpers)]

mod arch;
mod frame_allocator;
mod machine;
mod macros;
mod memory;
pub mod proptest;

pub use arch::EmulateArch;
pub use frame_allocator::TestFrameAllocator;
pub use machine::{Cpu, HasMemory, Machine, MachineBuilder, MissingMemory};
pub use memory::Memory;
