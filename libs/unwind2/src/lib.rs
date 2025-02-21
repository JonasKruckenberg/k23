// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![expect(internal_features, reason = "lang items")]
#![feature(
    core_intrinsics,
    rustc_attrs,
    used_with_arg,
    lang_items,
    naked_functions
)]
#![expect(tail_expr_drop_order, reason = "vetted")]

extern crate alloc;

mod arch;
mod eh_action;
mod eh_info;
mod error;
mod exception;
mod frame;
mod lang_items;
mod utils;

use crate::eh_action::{find_eh_action, EHAction};
use crate::exception::Exception;
use crate::lang_items::ensure_personality_stub;
use crate::utils::with_context;
use alloc::boxed::Box;
use core::any::Any;
use core::intrinsics;
use core::mem::ManuallyDrop;
use core::panic::UnwindSafe;
use core::ptr::addr_of_mut;
pub use eh_info::EhInfo;
pub use error::Error;
use fallible_iterator::FallibleIterator;
pub use frame::{Frame, FrameIter};

pub use arch::Registers;

pub(crate) type Result<T> = core::result::Result<T, Error>;

/// Begin unwinding the stack.
///
/// This will perform [`Drop`] cleanup and call [`catch_unwind`] handlers.
///
/// The provided `payload` argument will be passed in the `Err` variant returned by any [`catch_unwind`]
/// handlers that are encountered in the callstack.
///
/// # Errors
///
/// Returns an error if unwinding fails.
pub fn begin_unwind(payload: Box<dyn Any + Send>) -> Result<()> {
    with_context(|regs, pc| {
        let frames = FrameIter::from_registers(regs.clone(), pc);

        // TODO at this point libunwind *would* have a 2 phase unwinding process where we
        //  walk the stack once to find the closest exception handler and then a second time
        //  up to that handler calling the personality routine on the way to determine if we
        //  need to perform cleanup. Buuuuut since we rolled this all into one here, raise_exception_phase_1
        //  actually didn't do anything and unwinding appears to be correct even without so win??
        // raise_exception_phase_1(frames.clone())?;

        raise_exception_phase2(frames, Exception::wrap(payload))?;

        Ok(())
    })
}

// /// The first phase of stack unwinding, in this phase we walk the stack attempting to find the next
// /// closest
// fn raise_exception_phase_1(mut frames: FrameIter) -> Result<usize> {
//     while let Some(frame) = frames.next()? {
//         if frame
//             .personality()
//             .map(ensure_personality_stub)
//             .transpose()?
//             .is_none()
//         {
//             continue;
//         }
//
//         let Some(mut lsda) = frame.language_specific_data() else {
//             continue;
//         };
//
//         let eh_action = find_eh_action(&mut lsda, &frame)?;
//
//         match eh_action {
//             EHAction::None | EHAction::Cleanup(_) => continue,
//             EHAction::Catch(_) => {
//                 let handler_cfa = frame.sp() - usize::from(frame.is_signal_trampoline());
//
//                 return Ok(handler_cfa);
//             }
//         }
//     }
//
//     Err(Error::EndOfStack)
// }

fn raise_exception_phase2(mut frames: FrameIter, exception: *mut Exception) -> Result<()> {
    while let Some(mut frame) = frames.next()? {
        if frame
            .personality()
            .map(ensure_personality_stub)
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
            EHAction::Cleanup(lpad) | EHAction::Catch(lpad) => {
                frame.set_reg(arch::UNWIND_DATA_REG.0, exception as usize);
                frame.set_reg(arch::UNWIND_DATA_REG.1, 0);
                frame.set_reg(arch::RA, usize::try_from(lpad).unwrap());
                frame.adjust_stack_for_args();

                // Safety: this will set up the frame context necessary to transfer control to the
                // landing pad. Since that landing pad is generated by the Rust compiler there isn't
                // much we can do except hope and pray that the instruction pointer is correct.
                unsafe { frame.restore() }
            }
        }
    }

    Err(Error::EndOfStack)
}

/// Invokes the closure, capturing an unwind if one occurs.
///
/// This function returns `Ok` if no unwind occurred or `Err` with the payload passed to [`begin_unwind`].
///
/// You can think of this function as a `try-catch` expression and [`begin_unwind`] as the `throw`
/// counterpart.
///
/// The closure provided is required to adhere to the [`UnwindSafe`] trait to ensure that all captured
/// variables are safe to cross this boundary. The purpose of this bound is to encode the concept
/// of [exception safety] in the type system. Most usage of this function should not need to worry about
/// this bound as programs are naturally unwind safe without unsafe code. If it becomes a problem the
/// [`AssertUnwindSafe`] wrapper struct can be used to quickly assert that the usage here is indeed
/// unwind safe.
///
/// # Errors
///
/// Returns an error with the boxed panic payload when the provided closure panicked.
///
/// [exception safety]: https://github.com/rust-lang/rfcs/blob/master/text/1236-stabilize-catch-panic.md
/// [`UnwindSafe`]: core::panic::UnwindSafe
/// [`AssertUnwindSafe`]: core::panic::AssertUnwindSafe
pub fn catch_unwind<F, R>(f: F) -> core::result::Result<R, Box<dyn Any + Send + 'static>>
where
    F: FnOnce() -> R + UnwindSafe,
{
    union Data<F, R> {
        // when we start, this field holds the closure
        f: ManuallyDrop<F>,
        // when the closure completed successfully, this will hold the return
        r: ManuallyDrop<R>,
        // when the closure panicked this will hold the panic payload
        p: ManuallyDrop<Box<dyn Any + Send>>,
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

        // Safety: exception comes from the Rust intrinsic, not much we do other than trust it
        match unsafe { Exception::unwrap(exception.cast()) } {
            Ok(p) => data.p = ManuallyDrop::new(p),
            Err(err) => {
                log::error!("Failed to catch exception: {err:?}");
                arch::abort("Failed to catch exception");
            }
        }
    }

    let mut data = Data {
        f: ManuallyDrop::new(f),
    };
    let data_ptr = addr_of_mut!(data).cast::<u8>();

    // Safety: intrinsic call
    unsafe {
        if intrinsics::catch_unwind(do_call::<F, R>, data_ptr, do_catch::<F, R>) == 0 {
            Ok(ManuallyDrop::into_inner(data.r))
        } else {
            Err(ManuallyDrop::into_inner(data.p))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;

    #[test]
    fn begin_and_catch_roundtrip() {
        let res = catch_unwind(|| {
            begin_unwind(Box::new(42)).unwrap();
        })
        .map_err(|err| *err.downcast_ref::<i32>().unwrap());
        assert_eq!(res, Err(42));
    }
}
