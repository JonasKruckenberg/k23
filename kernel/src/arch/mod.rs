// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! rough arch interface outline
//!
//! - call_with_setjmp, setjmp, longjmp, JumpBuf, JumpBufStruct
//! - invalidate_range, is_kernel_address, AddressSpace, KERNEL_ASPACE_BASE, 
//!     USER_ASPACE_BASE, PAGE_SHIFT, CANONICAL_ADDRESS_MASK, PAGE_SIZE, DEFAULT_ASID
//! - init_early, per_hart_init_early, per_hart_init_late
//! - device::cpu::init, device::cpu::with_cpu_info
//! - park_hart, park_hart_timeout

cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
        pub use riscv64::*;
        pub use riscv::*;
    } else if #[cfg(target_arch = "aarch64")] {
        mod aarch64;
        pub use aarch64::*;
    } else if #[cfg(target_arch = "x86_64")] {
        mod x86_64;
        pub use x86_64::*;
    } else {
        compile_error!("Unsupported target architecture");
    }
}
