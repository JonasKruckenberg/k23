// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Non-local control flow primitives (`setjmp`/`longjmp`).
//!
//! # A word of caution
//!
//! These two functions are *the* most unsafe function in this codebase:
//! If used incorrectly, they will eat your stack and corrupt all of it. As a treat,
//! they are also super unintuitive and weird to use!
//!
//! YOU WILL USE THESE WRONG!
//! IF YOU READ THIS THINKING IT MIGHT BE A SOLUTION TO YOUR PROBLEM: ITS NOT!
//!
//! `setjmp` saves important register state at the time of its calling into the provided `JumpBuf`
//! and `longjmp` will restore that register state.
//! This essentially allows you to perform returns to arbitrary frames on the stack. (it doesn't even
//! need to be your stack for funsies).
//! The way this manifests is in `setjmp` returning zero the first time, indicating the register state
//! got saved. And then, whenever `longjmp` is called, control flow disappears from that codepath
//! (`longjmp` returns `!`) and *magically* reappears as **another return of the `setjpm` function**.
//! (called a ghost return).
//!
//! I don't think I need to explain further why these two functions are unsafe and weird do I?
//!
//! # Why does this exist at all?
//!
//! `setjmp`/`longjmp` are basically "we have stack unwinding at home" they allow you to skip up
//! many stack frames at once. Note that in addition to the unsafety mentioned above, `longjmp` also
//! *does not call drop handlers* any resources that need explicit drop handling are leaked.
//!
//! These two functions exist in k23 for one reason: Unlike stack unwinding they allow us to skip
//! over JIT-code created frames easily. Whenever a trap is taken in WASM JIT code, we *could* begin
//! stack unwinding, but our unwinder doesn't know how to unwind the WASM stack, the DWARF info it uses
//! only covers the Rust code.
//!
//! Using `setjmp`/`longjmp` this way might be the only sound way to do it, we actually never longjmp
//! past native Rust frames, instead at each `host->wasm` boundary we convert the trap into a regular Rust
//! result. In a nested calls scenario (e.g. host->wasm->host->wasm) it is therefore up to each host function
//! to propagate the trap and each host function therefore gets to clean up all its resources.

use super::utils::{define_op, load_fp, load_gp, save_fp, save_gp};
use core::arch::{asm, naked_asm};
use core::marker::{PhantomData, PhantomPinned};
use core::mem::{ManuallyDrop, MaybeUninit};
use core::ptr::addr_of_mut;

/// A store for the register state used by `setjmp` and `longjmp`.
///
/// In essence this marks a "checkpoint" in the program's execution that can be returned to later.
#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct JmpBufStruct {
    pc: usize,
    s: [usize; 12],
    sp: usize,
    fs: [usize; 12],
    _neither_send_nor_sync: PhantomData<*const u8>,
    _not_unpin: PhantomPinned,
}

pub type JmpBuf = *const JmpBufStruct;

/// Saves various information about the calling environment (the stack pointer,
/// the instruction pointer and callee saved registers) and establishes a "checkpoint"
/// to which control flow can later be transferred.
///
/// This function pretty weird, it can return more than one time:
/// - The first time it returns, the return value is `0` indicating that the context has been saved.
/// - Subsequently, calls to `longjmp` that transfer control to the `*mut JumpBuf` used by this `setjmp`
///     will cause this function to return again, this time with the value passed to `longjmp`.
///
/// This implementation has been adapted from the [LLVM libc implementation (Apache License v2.0 with LLVM Exceptions)](https://github.com/llvm/llvm-project/blob/bbf2ad026eb0b399364a889799ef6b45878cd299/libc/src/setjmp/riscv/setjmp.cpp)
///
/// # Safety
///
/// Due to the weird multi-return nature of `setjmp` it is very easy to make mistakes, this
/// function be used with extreme care.
#[naked]
pub unsafe extern "C" fn setjmp(env: JmpBuf) -> isize {
    // Safety: inline assembly
    unsafe {
        cfg_if::cfg_if! {
            if #[cfg(target_feature = "d")] {
                naked_asm! {
                    save_gp!(ra => a0[0]),
                    save_gp!(s0 => a0[1]),
                    save_gp!(s1 => a0[2]),
                    save_gp!(s2 => a0[3]),
                    save_gp!(s3 => a0[4]),
                    save_gp!(s4 => a0[5]),
                    save_gp!(s5 => a0[6]),
                    save_gp!(s6 => a0[7]),
                    save_gp!(s7 => a0[8]),
                    save_gp!(s8 => a0[9]),
                    save_gp!(s9 => a0[10]),
                    save_gp!(s10 => a0[11]),
                    save_gp!(s11 => a0[12]),
                    save_gp!(sp => a0[13]),

                    save_fp!(fs0 => a0[14]),
                    save_fp!(fs1 => a0[15]),
                    save_fp!(fs2 => a0[16]),
                    save_fp!(fs3 => a0[17]),
                    save_fp!(fs4 => a0[18]),
                    save_fp!(fs5 => a0[19]),
                    save_fp!(fs6 => a0[20]),
                    save_fp!(fs7 => a0[21]),
                    save_fp!(fs8 => a0[22]),
                    save_fp!(fs9 => a0[23]),
                    save_fp!(fs10 => a0[24]),
                    save_fp!(fs11 => a0[25]),

                    "mv a0, zero",
                    "ret",
                }
            } else {
                naked_asm! {
                    save_gp!(ra => a0[0]),
                    save_gp!(s0 => a0[1]),
                    save_gp!(s1 => a0[2]),
                    save_gp!(s2 => a0[3]),
                    save_gp!(s3 => a0[4]),
                    save_gp!(s4 => a0[5]),
                    save_gp!(s5 => a0[6]),
                    save_gp!(s6 => a0[7]),
                    save_gp!(s7 => a0[8]),
                    save_gp!(s8 => a0[9]),
                    save_gp!(s9 => a0[10]),
                    save_gp!(s10 => a0[11]),
                    save_gp!(s11 => a0[12]),
                    save_gp!(sp => a0[13]),
                    "mv a0, zero",
                    "ret",
                }
            }
        }
    }
}

/// Performs a non-local jump to a context previously saved by `setjmp`.
///
/// This implementation has been adapted from the [LLVM libc implementation (Apache License v2.0 with LLVM Exceptions)](https://github.com/llvm/llvm-project/blob/1ae0dae368e4bbf2177603d5c310e794c4fd0bd8/libc/src/setjmp/riscv/longjmp.cpp)
///
/// # Safety
///
/// This will transfer control to instructions saved in the `*mut JumpBuf` parameter,
/// so extreme care must be taken to ensure that the `JumpBuf` is valid and has been initialized.
/// Additionally, the whole point of this function is to continue execution at a wildly different
/// address, so this might cause confusing and hard-to-debug behavior.
#[naked]
pub unsafe extern "C" fn longjmp(env: JmpBuf, val: isize) -> ! {
    // Safety: inline assembly
    unsafe {
        cfg_if::cfg_if! {
            if #[cfg(target_feature = "d")] {
                naked_asm! {
                    load_gp!(a0[0] => ra),
                    load_gp!(a0[1] => s0),
                    load_gp!(a0[2] => s1),
                    load_gp!(a0[3] => s2),
                    load_gp!(a0[4] => s3),
                    load_gp!(a0[5] => s4),
                    load_gp!(a0[6] => s5),
                    load_gp!(a0[7] => s6),
                    load_gp!(a0[8] => s7),
                    load_gp!(a0[9] => s8),
                    load_gp!(a0[10] => s9),
                    load_gp!(a0[11] => s10),
                    load_gp!(a0[12] => s11),
                    load_gp!(a0[13] => sp),

                    load_fp!(a0[14] => fs0),
                    load_fp!(a0[15] => fs1),
                    load_fp!(a0[16] => fs2),
                    load_fp!(a0[17] => fs3),
                    load_fp!(a0[18] => fs4),
                    load_fp!(a0[19] => fs5),
                    load_fp!(a0[20] => fs6),
                    load_fp!(a0[21] => fs7),
                    load_fp!(a0[22] => fs8),
                    load_fp!(a0[23] => fs9),
                    load_fp!(a0[24] => fs10),
                    load_fp!(a0[25] => fs11),

                    "add a0, a1, zero",
                    "ret",
                }
            } else {
                naked_asm! {
                    load_gp!(a0[0] => ra),
                    load_gp!(a0[1] => s0),
                    load_gp!(a0[2] => s1),
                    load_gp!(a0[3] => s2),
                    load_gp!(a0[4] => s3),
                    load_gp!(a0[5] => s4),
                    load_gp!(a0[6] => s5),
                    load_gp!(a0[7] => s6),
                    load_gp!(a0[8] => s7),
                    load_gp!(a0[9] => s8),
                    load_gp!(a0[10] => s9),
                    load_gp!(a0[11] => s10),
                    load_gp!(a0[12] => s11),
                    load_gp!(a0[13] => sp),

                    "add a0, a1, zero",
                    "ret",
                }
            }
        }
    }
}

/// Invokes a closure, setting up the environment for contained code to safely use `longjmp`.
///
/// This function acts as a somewhat-safe wrapper around `setjmp` that prevents LLVM miscompilations
/// caused by the fact that its optimization passes don't know about the *returns-twice* nature of `setjmp`.
///
/// Note for the pedantic: Yes LLVM *could* know about this, and does have logic to handle it, but Rust
/// has (sensibly) decided to remove the `returns_twice` attribute from the language, so instead
/// we have to rely on this wrapper.
///
/// # Safety
///
/// While `longjmp` is still very sketchy, skipping over destructors and such, this function does
/// the necessary ceremony to ensure safe, Rust compatible usage of `setjmp`. In particular, it ensures
/// that the `JmpBuf` cannot be leaked out of the closure, and that it cannot be shared between
/// threads.
// The code below is adapted from https://github.com/pnkfelix/cee-scape/blob/d6ffeca6bd56b46b83c8c9118dbe75e38d423d28/src/asm_based.rs
// which in turn is adapted from this Zulip thread: https://rust-lang.zulipchat.com/#narrow/stream/210922-project-ffi-unwind/topic/cost.20of.20supporting.20longjmp.20without.20annotations/near/301840755
#[inline(never)]
pub fn call_with_setjmp<F>(f: F) -> isize
where
    F: for<'a> FnOnce(&'a JmpBufStruct) -> isize,
{
    extern "C" fn do_call<F>(env: JmpBuf, closure_env_ptr: *mut F) -> isize
    where
        F: for<'a> FnOnce(&'a JmpBufStruct) -> isize,
    {
        // Dereference `closure_env_ptr` with .read() to acquire ownership of
        // the FnOnce object, then call it. (See also the forget note below.)
        //
        // Note that `closure_env_ptr` is not a raw function pointer, it's a
        // pointer to a FnOnce; the code we call comes from the generic `F`.
        //
        // Safety: caller has to ensure ptr is valid
        unsafe { closure_env_ptr.read()(&*env) }
    }

    // Safety: inline assembly
    unsafe {
        let mut f = ManuallyDrop::new(f);
        let mut jbuf = MaybeUninit::<JmpBufStruct>::zeroed().assume_init();
        let ret: isize;
        let env_ptr = addr_of_mut!(jbuf);
        let closure_ptr = addr_of_mut!(f);

        asm! {
            "call {setjmp}",        // fills in jbuf; future longjmp calls go here.
            "bnez a0, 1f",          // if return value non-zero, skip the callback invocation
            "mv a0, {env_ptr}",     // move saved jmp buf into do_call's first arg position
            "mv a1, {closure_ptr}", // move saved closure env into do_call's second arg position
            "call {do_call}",       // invoke the do_call and through it the callback
            "1:",                   // at this point, a0 carries the return value (from either outcome)

            in("a0") env_ptr,
            setjmp = sym setjmp,
            do_call = sym do_call::<F>,
            env_ptr = in(reg) env_ptr,
            closure_ptr = in(reg) closure_ptr,
            lateout("a0") ret,
            clobber_abi("C")
        }
        ret
    }
}

#[cfg(test)]
mod tests {
    // TODO reenable with test runner
    // #[ktest::test]
    // fn _call_with_setjmp() {
    //     unsafe {
    //         let ret = call_with_setjmp(|_env| 1234);
    //         assert_eq!(ret, 1234);
    //
    //         let ret = call_with_setjmp(|env| {
    //             longjmp(env, 4321);
    //         });
    //         assert_eq!(ret, 4321);
    //     }
    // }

    // TODO reenable with test runner
    // #[ktest::test]
    // #[allow(static_mut_refs)]
    // fn setjmp_longjmp_simple() {
    //     // The LLVM optimizer doesn't understand the "setjmp returns twice" behaviour and would
    //     // turn the `C += 1` into a constant store instruction instead of a load-add-store sequence.
    //     // To force this, we use a static variable here, but forcing the location of the variable
    //     // into a different (longer-lived) stack frame would also work.
    //     //
    //     // Note that this only exists to test the behaviour of setjmp/longjmp, in real code you
    //     // should use `call_with_setjmp` as it limits much of the footguns.
    //
    //     static mut C: u32 = 0;
    //
    //     unsafe {
    //         let mut buf = MaybeUninit::<JmpBufStruct>::zeroed().assume_init();
    //
    //         let r = setjmp(ptr::from_mut(&mut buf));
    //         C += 1;
    //         if r == 0 {
    //             assert_eq!(C, 1);
    //             longjmp(ptr::from_mut(&mut buf), 1234567);
    //         }
    //         assert_eq!(C, 2);
    //         assert_eq!(r, 1234567);
    //     }
    // }

    // static mut BUFFER_A: JmpBufStruct =
    //     unsafe { MaybeUninit::<JmpBufStruct>::zeroed().assume_init() };
    // static mut BUFFER_B: JmpBufStruct =
    //     unsafe { MaybeUninit::<JmpBufStruct>::zeroed().assume_init() };

    // TODO reenable with test runner
    // #[ktest::test]
    // fn setjmp_longjmp_complex() {
    //     unsafe fn routine_a() {
    //         let r = setjmp(addr_of_mut!(BUFFER_A));
    //         if r == 0 {
    //             routine_b()
    //         }
    //         assert_eq!(r, 10001);
    //
    //         let r = setjmp(addr_of_mut!(BUFFER_A));
    //         if r == 0 {
    //             longjmp(addr_of_mut!(BUFFER_B), 20001);
    //         }
    //         assert_eq!(r, 10002);
    //
    //         let r = setjmp(addr_of_mut!(BUFFER_A));
    //         if r == 0 {
    //             longjmp(addr_of_mut!(BUFFER_B), 20002);
    //         }
    //         debug_assert!(r == 10003);
    //     }
    //
    //     unsafe fn routine_b() {
    //         let r = setjmp(addr_of_mut!(BUFFER_B));
    //         if r == 0 {
    //             longjmp(addr_of_mut!(BUFFER_A), 10001);
    //         }
    //         assert_eq!(r, 20001);
    //
    //         let r = setjmp(addr_of_mut!(BUFFER_B));
    //         if r == 0 {
    //             longjmp(addr_of_mut!(BUFFER_A), 10002);
    //         }
    //         assert_eq!(r, 20002);
    //
    //         let r = setjmp(addr_of_mut!(BUFFER_B));
    //         if r == 0 {
    //             longjmp(addr_of_mut!(BUFFER_A), 10003);
    //         }
    //     }
    //
    //     unsafe {
    //         routine_a();
    //     }
    // }
}
