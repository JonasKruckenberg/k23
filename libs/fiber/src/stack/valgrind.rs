// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Optional support for registering stacks with Valgrind.
//!
//! When running under Valgrind, we need to notify it when we allocate/free a
//! stack otherwise it gets confused when the stack pointer starts to randomly
//! move to a different address.
//!
//! This is done through special instruction sequences which are recognized by
//! Valgrind but otherwise executes as a NOP on real hardware.

cfg_if::cfg_if! {
    if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
        type Value = usize;

        // Valgrind doesn't support RISC-V yet, use a no-op for now.
        #[inline]
        unsafe fn valgrind_request(default: Value, _args: &[Value; 6]) -> Value {
            default
        }
    } else if #[cfg(target_arch = "aarch64")] {
        type Value = u64;

        #[inline]
        unsafe fn valgrind_request(default: Value, args: &[Value; 6]) -> Value {
            let result;
            // Safety: inline assembly
            unsafe {
                core::arch::asm!(
                    "ror x12, x12, #3",
                    "ror x12, x12, #13",
                    "ror x12, x12, #61",
                    "ror x12, x12, #51",
                    "orr x10, x10, x10",
                    inout("x3") default => result,
                    in("x4") args.as_ptr(),
                    options(nostack),
                );
            }
            result
        }
    } else if #[cfg(target_arch = "x86_64")] {
        type Value = u64;

        #[inline]
        unsafe fn valgrind_request(default: Value, args: &[Value; 6]) -> Value {
            let result;
            // Safety: inline assembly
            unsafe {
                core::arch::asm!(
                    "rol rdi, 3",
                    "rol rdi, 13",
                    "rol rdi, 61",
                    "rol rdi, 51",
                    "xchg rbx, rbx",
                    inout("rdx") default => result,
                    in("rax") args.as_ptr(),
                    options(nostack),
                );
            }
            result
        }
    } else {
        compile_error!("Unsupported target architecture");
    }
}

const STACK_REGISTER: Value = 0x1501;
const STACK_DEREGISTER: Value = 0x1502;

/// Helper type which registers a stack with Valgrind and automatically
/// de-registers it when dropped.
///
/// This has no effect when not running under Valgrind.
#[derive(Debug)]
pub struct ValgrindStackRegistration {
    id: Value,
}

impl ValgrindStackRegistration {
    /// Registers the given region of memory as a stack so that Valgrind can
    /// properly recognize legitimate stack switches.
    #[inline]
    pub fn new(addr: *mut u8, len: usize) -> Self {
        Self {
            id: unsafe {
                valgrind_request(
                    0,
                    &[
                        STACK_REGISTER,
                        addr as Value,
                        addr as Value + len as Value,
                        0,
                        0,
                        0,
                    ],
                )
            },
        }
    }
}

impl Drop for ValgrindStackRegistration {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            valgrind_request(0, &[STACK_DEREGISTER, self.id, 0, 0, 0, 0]);
        }
    }
}
