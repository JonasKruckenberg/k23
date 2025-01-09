// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![allow(internal_features)]
#![feature(
    core_intrinsics,
    rustc_attrs,
    used_with_arg,
    lang_items,
    naked_functions
)]

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
pub use frame::{Frame, FramesIter};

pub(crate) type Result<T> = core::result::Result<T, Error>;

pub fn begin_panic(data: Box<dyn Any + Send>) -> Result<()> {
    with_context(|ctx| {
        raise_exception_phase_1(ctx.clone())?;

        raise_exception_phase2(ctx.clone(), Exception::wrap(data))?;

        Ok(())
    })
}

fn raise_exception_phase_1(ctx: arch::Context) -> Result<usize> {
    let mut frames = FramesIter::from_context(ctx);

    while let Some(frame) = frames.next()? {
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
            EHAction::None | EHAction::Cleanup(_) => continue,
            EHAction::Catch(_) => {
                let handler_cfa = frame.sp() - usize::from(frame.is_signal_trampoline());

                return Ok(handler_cfa);
            }
        }
    }

    Err(Error::EndOfStack)
}

fn raise_exception_phase2(ctx: arch::Context, exception: *mut Exception) -> Result<()> {
    let mut frames = FramesIter::from_context(ctx);

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
            EHAction::Cleanup(lpad) | EHAction::Catch(lpad) => unsafe {
                frame.set_reg(arch::UNWIND_DATA_REG.0, exception as usize);
                frame.set_reg(arch::UNWIND_DATA_REG.1, 0);
                frame.set_ip(lpad as usize);
                frame.adjust_stack_for_args();
                frame.restore()
            },
        }
    }

    Err(Error::EndOfStack)
}

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
        unsafe {
            let data = data.cast::<Data<F, R>>();
            let data = &mut (*data);

            match Exception::unwrap(exception.cast()) {
                Ok(p) => data.p = ManuallyDrop::new(p),
                Err(err) => {
                    log::error!("Failed to catch exception: {err:?}");
                    arch::abort();
                }
            }
        }
    }

    let mut data = Data {
        f: ManuallyDrop::new(f),
    };
    let data_ptr = addr_of_mut!(data).cast::<u8>();

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
            begin_panic(Box::new(42)).unwrap();
        })
        .map_err(|err| *err.downcast_ref::<i32>().unwrap());
        assert_eq!(res, Err(42));
    }
}
