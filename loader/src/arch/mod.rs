// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::frame_alloc::FrameAllocator;
use crate::mapping::Flags;
use core::num::NonZero;

cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
        pub use riscv64::*;
        pub use riscv::*;
    } else {
        compile_error!("Unsupported target architecture");
    }
}

pub fn abort() -> ! {
    cfg_if::cfg_if! {
        if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            riscv::abort();
        } else {
            loop {}
        }
    }
}

pub(crate) unsafe fn map_contiguous(
    p0: &mut FrameAllocator,
    p1: usize,
    p2: usize,
    p3: NonZero<usize>,
    p4: Flags,
) -> crate::Result<()> {
    todo!()
}

pub(crate) unsafe fn remap_contiguous(
    p0: usize,
    p1: usize,
    p2: NonZero<usize>,
) -> crate::Result<()> {
    todo!()
}

pub(crate) unsafe fn activate_aspace(p0: usize) {
    todo!()
}
