// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std] // this is crate is fully incompatible with `std` due to clashing lang item definitions
#![cfg(target_os = "none")]
#![expect(internal_features, reason = "lang items")]
#![feature(core_intrinsics, rustc_attrs, used_with_arg, lang_items, never_type)]

mod arch;
mod eh_action;
mod eh_info;
mod error;
mod exception;
mod frame;
mod lang_items;
mod utils;

use core::intrinsics;
use core::mem::ManuallyDrop;
use core::panic::UnwindSafe;
use core::ptr::addr_of_mut;

use abort::abort;
pub use arch::{Registers, restore_context, save_context};
use eh_action::{EHAction, find_eh_action};
pub use eh_info::EhInfo;
pub use error::Error;
pub use exception::UnwindException;
use fallible_iterator::FallibleIterator;
pub use frame::{Frame, FrameIter};
use lang_items::ensure_rust_personality_routine;
pub use utils::with_context;

pub(crate) type Result<T> = core::result::Result<T, Error>;

/// Begin unwinding *a* stack. The specific stack location at which unwinding will begin is determined
/// by the register set and program counter provided.
///
/// Unwinding will walk up the stack, calling [`Drop`] handlers along the way to perform cleanup until
/// it reaches a [`catch_unwind`] handler.
///
/// When reached, control is transferred to the [`catch_unwind`] handler, which receives the
/// `exception` pointer in the `Err` variant of its return. In that case, this function will *not*
/// return.
///
/// The `exception` pointer is opaque to this crate: it is threaded unchanged through the landing pad
/// and handed back to the catcher. Callers own whatever it points at.
///
/// In this kernel, raise only through `sys/panic-unwind` so that panic accounting stays balanced
/// against [`catch_unwind`].
///
/// # Errors
///
/// If there is no [`catch_unwind`] handler anywhere in the call chain then this function returns
/// `Err(Error::EndOfStack)`. This roughly equivalent to an uncaught exception in C++ and should
/// be treated as a fatal error.
///
/// # Safety
///
/// `exception` must point at a live [`UnwindException`] that outlives the unwind, and the register
/// values are not checked — if they are incorrect this might lead to segfaults.
pub unsafe fn begin_unwind_with(
    exception: *mut UnwindException,
    regs: Registers,
    pc: usize,
) -> Result<!> {
    let frames = FrameIter::from_registers(regs, pc);

    raise_exception_phase2(frames, exception)
}

/// Walk up the stack until either a landing pad is encountered or we reach the end of the stack.
///
/// If a landing pad is found control is transferred to it and this function will not return, if there
/// is no landing pad, this function will return `Err(Error::EndOfStack)`.
///
/// Note that the traditional unwinding process has 2 phases, the first where the landing pad is discovered
/// and the second where the stack is actually unwound up to that landing pad.
/// In `unwind` we can get away with one phase because we bypass the language personality routine:
/// Traditional unwinders call the personality routine on each frame to discover a landing pad, and
/// then during cleanup call the personality routine again to determine if control should actually be
/// transferred. This is done so that languages have maximum flexibility in how they treat exceptions.
///
/// `unwind` - being Rust-only - doesn't need that flexibility since Rust landing pads are called
/// unconditionally. Furthermore, `unwind` never actually calls the personality routine, instead
/// parsing the [`EHAction`] for each frame directly.
///
/// The name `raise_exception_phase2` is kept though to make it easier to understand what this function
/// does when coming from traditional unwinders.
fn raise_exception_phase2(mut frames: FrameIter, exception: *mut UnwindException) -> Result<!> {
    while let Some(mut frame) = frames.next()? {
        if frame
            .personality()
            .map(ensure_rust_personality_routine)
            .transpose()?
            .is_none()
        {
            continue;
        }

        let Some(mut lsda) = frame.language_specific_data() else {
            continue;
        };

        let eh_action = find_eh_action(&mut lsda, &frame)?;

        match eh_action {
            EHAction::None => continue,
            // Safety: As long as the Rust compiler works correctly lpad is the correct instruction
            // pointer.
            EHAction::Cleanup(lpad) | EHAction::Catch(lpad) | EHAction::Filter(lpad) => {
                frame.set_reg(arch::UNWIND_DATA_REG.0, exception as usize);
                frame.set_reg(arch::UNWIND_DATA_REG.1, 0);
                frame.set_reg(arch::RA, lpad as usize);
                frame.adjust_stack_for_args();

                // Safety: this will set up the frame context necessary to transfer control to the
                // landing pad. Since that landing pad is generated by the Rust compiler there isn't
                // much we can do except hope and pray that the instruction pointer is correct.
                unsafe { frame.restore() }
            }
            EHAction::Terminate => {}
        }
    }

    log::trace!("reached end of stack without handler");
    Err(Error::EndOfStack)
}

/// Invokes the closure, capturing an unwind if one occurs.
///
/// This function returns `Ok` if no unwind occurred or `Err` with the in-flight exception pointer.
///
/// You can think of this function as a `try-catch` expression and [`begin_unwind_with`] as the `throw`
/// counterpart.
///
/// The closure provided is required to adhere to the [`UnwindSafe`] trait to ensure that all captured
/// variables are safe to cross this boundary. The purpose of this bound is to encode the concept
/// of [exception safety] in the type system. Most usage of this function should not need to worry about
/// this bound as programs are naturally unwind safe without unsafe code. If it becomes a problem the
/// [`AssertUnwindSafe`] wrapper struct can be used to quickly assert that the usage here is indeed
/// unwind safe.
///
/// This function is payload-agnostic: on an unwind it returns the raw [`UnwindException`] pointer
/// the raiser threaded through, leaving it to the caller to recover whatever payload sits alongside
/// that header.
///
/// # Errors
///
/// Returns the in-flight [`UnwindException`] pointer when the provided closure unwound.
///
/// [exception safety]: https://github.com/rust-lang/rfcs/blob/master/text/1236-stabilize-catch-panic.md
/// [`UnwindSafe`]: core::panic::UnwindSafe
/// [`AssertUnwindSafe`]: core::panic::AssertUnwindSafe
pub fn catch_unwind<F, R>(f: F) -> core::result::Result<R, *mut UnwindException>
where
    F: FnOnce() -> R + UnwindSafe,
{
    union Data<F, R> {
        // when we start, this field holds the closure
        f: ManuallyDrop<F>,
        // when the closure completed successfully, this will hold the return
        r: ManuallyDrop<R>,
        // when the closure unwound this will hold the in-flight exception pointer
        p: *mut UnwindException,
    }

    #[inline]
    fn do_call<F: FnOnce() -> R, R>(data: *mut u8) {
        // SAFETY: this is the responsibility of the caller, see above.
        unsafe {
            let data = data.cast::<Data<F, R>>();
            let data = &mut (*data);
            let f = ManuallyDrop::take(&mut data.f);
            data.r = ManuallyDrop::new(f());
        }
    }

    #[cold]
    #[rustc_nounwind] // `intrinsic::catch_unwind` requires catch fn to be nounwind
    fn do_catch<F: FnOnce() -> R, R>(data: *mut u8, exception: *mut u8) {
        let data = data.cast::<Data<F, R>>();
        // Safety: data is correctly initialized
        let data = unsafe { &mut (*data) };

        #[expect(
            clippy::cast_ptr_alignment,
            reason = "the intrinsic types the raised `UnwindException` (align 16) as `*mut u8`; it is already correctly aligned"
        )]
        let exception = exception.cast::<UnwindException>();
        // Safety: `exception` comes from the Rust unwind intrinsic and points at a live header.
        // A mismatched class means an exception from a runtime that cannot exist in-kernel reached
        // us — a bug, not something to recover from.
        if !unsafe { (*exception).is_rust() } {
            log::error!("caught an exception with a foreign class. aborting.");
            abort();
        }
        data.p = exception;
    }

    let mut data = Data {
        f: ManuallyDrop::new(f),
    };
    let data_ptr = addr_of_mut!(data).cast::<u8>();

    // Safety: intrinsic call
    unsafe {
        if intrinsics::catch_unwind(do_call::<F, R>, data_ptr, do_catch::<F, R>) {
            Err(data.p)
        } else {
            Ok(ManuallyDrop::into_inner(data.r))
        }
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;
    use core::panic::AssertUnwindSafe;

    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::util::SubscriberInitExt;

    use super::*;

    extern crate std;

    /// Raise `exception` from the current stack location and drive the unwinder.
    /// Diverges: control transfers to the nearest [`catch_unwind`] landing pad.
    fn raise(exception: *mut UnwindException) -> ! {
        with_context(|regs, pc| {
            // Safety: `exception` outlives the unwind and `regs`/`pc` were just captured here.
            unsafe { begin_unwind_with(exception, regs.clone(), pc).unwrap() }
        })
    }

    #[test]
    fn begin_and_catch_roundtrip() {
        let _trace = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .set_default();

        let mut exception = UnwindException::new();
        let exception_ptr: *mut UnwindException = &raw mut exception;

        let caught = catch_unwind(AssertUnwindSafe(|| -> () { raise(exception_ptr) })).unwrap_err();
        assert_eq!(caught, exception_ptr);
    }

    /// A nested unwind raised from a cleanup pad of an outer unwind must be
    /// caught before the outer unwind resumes, and must not disturb the outer
    /// exception the outer catcher ultimately receives. This is the in-flight
    /// LIFO property that lets `panic-unwind` share one immutable exception header.
    #[test]
    fn nested_catch_during_cleanup() {
        let _trace = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .set_default();

        let mut outer = UnwindException::new();
        let mut inner = UnwindException::new();
        let outer_ptr: *mut UnwindException = &raw mut outer;
        let inner_ptr: *mut UnwindException = &raw mut inner;

        let inner_caught = Cell::new(false);

        // Dropped while the outer unwind walks the frame, i.e. inside a cleanup
        // pad. Raising a fresh exception here nests inside the outer one.
        struct Nested<'a> {
            inner_ptr: *mut UnwindException,
            inner_caught: &'a Cell<bool>,
        }
        impl Drop for Nested<'_> {
            fn drop(&mut self) {
                let ptr = self.inner_ptr;
                let caught = catch_unwind(AssertUnwindSafe(|| -> () { raise(ptr) })).unwrap_err();
                self.inner_caught.set(caught == ptr);
            }
        }

        let outer_caught = catch_unwind(AssertUnwindSafe(|| -> () {
            let _nested = Nested {
                inner_ptr,
                inner_caught: &inner_caught,
            };
            raise(outer_ptr)
        }))
        .unwrap_err();

        assert!(
            inner_caught.get(),
            "inner exception not caught during cleanup"
        );
        assert_eq!(
            outer_caught, outer_ptr,
            "outer exception not delivered intact after nested unwind"
        );
    }
}
