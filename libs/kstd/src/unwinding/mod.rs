mod arch;
mod frame;

use core::{
    ffi::{c_int, c_void},
    ptr,
};
use frame::Frame;

pub struct UnwindContext<'a> {
    frame: Option<&'a Frame<'a>>,
    ctx: &'a mut arch::Context,
    signal: bool,
}

#[repr(C)]
pub struct UnwindException {
    pub exception_class: u64,
    pub exception_cleanup: Option<UnwindExceptionCleanupFn>,
}

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct UnwindReasonCode(pub c_int);

#[allow(unused)]
impl UnwindReasonCode {
    pub const NO_REASON: Self = Self(0);
    pub const FOREIGN_EXCEPTION_CAUGHT: Self = Self(1);
    pub const FATAL_PHASE2_ERROR: Self = Self(2);
    pub const FATAL_PHASE1_ERROR: Self = Self(3);
    pub const NORMAL_STOP: Self = Self(4);
    pub const END_OF_STACK: Self = Self(5);
    pub const HANDLER_FOUND: Self = Self(6);
    pub const INSTALL_CONTEXT: Self = Self(7);
    pub const CONTINUE_UNWIND: Self = Self(8);
}

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct UnwindAction(pub c_int);

impl UnwindAction {
    pub const SEARCH_PHASE: Self = Self(1);
    pub const CLEANUP_PHASE: Self = Self(2);
    pub const HANDLER_FRAME: Self = Self(4);
    pub const FORCE_UNWIND: Self = Self(8);
    pub const END_OF_STACK: Self = Self(16);
}

pub type PersonalityRoutine = unsafe extern "C" fn(
    // version
    c_int,
    UnwindAction,
    // exception_class
    u64,
    *mut UnwindException,
    &mut UnwindContext<'_>,
) -> UnwindReasonCode;

pub type UnwindExceptionCleanupFn = unsafe extern "C" fn(UnwindReasonCode, *mut UnwindException);

pub type UnwindStopFn = unsafe extern "C" fn(
    // version
    c_int,
    UnwindAction,
    // exception_class
    u64,
    *mut UnwindException,
    &mut UnwindContext<'_>,
    *mut c_void,
) -> UnwindReasonCode;

#[inline(never)]
#[no_mangle]
pub unsafe extern "C-unwind" fn _Unwind_RaiseException(
    exception: *mut UnwindException,
) -> UnwindReasonCode {
    with_context(|ctx| {
        let mut signal = false;
        loop {
            let frame = match Frame::from_context(ctx, signal) {
                Ok(Some(frame)) => frame,
                Ok(None) => {
                    return UnwindReasonCode::END_OF_STACK;
                }
                Err(_) => {
                    return UnwindReasonCode::FATAL_PHASE1_ERROR;
                }
            };

            if let Some(personality) = frame.personality() {
                let result = unsafe {
                    personality(
                        1,
                        UnwindAction::SEARCH_PHASE,
                        (*exception).exception_class,
                        exception,
                        &mut UnwindContext {
                            frame: Some(&frame),
                            ctx,
                            signal,
                        },
                    )
                };

                match result {
                    UnwindReasonCode::CONTINUE_UNWIND => (),
                    UnwindReasonCode::HANDLER_FOUND => {
                        break;
                    }
                    _ => return UnwindReasonCode::FATAL_PHASE1_ERROR,
                }
            }

            ctx = frame.unwind(ctx).unwrap();
            signal = frame.is_signal_trampoline();
        }
    })
}

// phase 1: find the handler
fn raise_exception_phase1() {}
fn raise_exception_phase2() {}

#[inline(never)]
#[no_mangle]
pub unsafe extern "C-unwind" fn _Unwind_ForcedUnwind(
    exception: *mut UnwindException,
    stop: UnwindStopFn,
    stop_arg: *mut c_void,
) -> UnwindReasonCode {
    todo!()
}

fn forced_unwind_phase1() {}
fn forced_unwind_phase2() {}

#[inline(never)]
#[no_mangle]
pub unsafe extern "C-unwind" fn _Unwind_Resume(exception: *mut UnwindException) -> ! {
    todo!()
}

// Helper function to turn `save_context` which takes function pointer to a closure-taking function.
fn with_context<T, F: FnOnce(&mut arch::Context) -> T>(f: F) -> T {
    use core::mem::ManuallyDrop;

    union Data<T, F> {
        f: ManuallyDrop<F>,
        t: ManuallyDrop<T>,
    }

    extern "C" fn delegate<T, F: FnOnce(&mut arch::Context) -> T>(
        ctx: &mut arch::Context,
        ptr: *mut (),
    ) {
        // SAFETY: This function is called exactly once; it extracts the function, call it and
        // store the return value. This function is `extern "C"` so we don't need to worry about
        // unwinding past it.
        unsafe {
            let data = &mut *ptr.cast::<Data<T, F>>();
            let t = ManuallyDrop::take(&mut data.f)(ctx);
            data.t = ManuallyDrop::new(t);
        }
    }

    let mut data = Data {
        f: ManuallyDrop::new(f),
    };
    arch::save_context(delegate::<T, F>, ptr::addr_of_mut!(data).cast());
    unsafe { ManuallyDrop::into_inner(data.t) }
}
