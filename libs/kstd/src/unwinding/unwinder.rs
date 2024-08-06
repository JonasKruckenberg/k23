#![allow(clippy::missing_safety_doc, clippy::missing_panics_doc)]

use super::{frame::Frame, utils::with_context};
use crate::arch;
use bitflags::bitflags;
use core::{
    ffi::{c_int, c_void},
    fmt, ptr,
};
use gimli::Register;

#[derive(Debug, onlyerror::Error)]
enum UnwindError {
    #[error("failed to construct frame {0:?}")]
    ConstructFrame(gimli::Error),
    #[error("failed to unwind frame {0:?}")]
    UnwindFrame(gimli::Error),
    #[error("end of stack")]
    EndOfStack,
    #[error("personality routine failed with reason code {0:?}")]
    PersonalityFailure(UnwindReasonCode),
}

impl UnwindError {
    pub fn into_phase1_reason_code(self) -> UnwindReasonCode {
        match self {
            Self::ConstructFrame(_) | Self::UnwindFrame(_) => UnwindReasonCode::FATAL_PHASE1_ERROR,
            Self::EndOfStack => UnwindReasonCode::END_OF_STACK,
            Self::PersonalityFailure(code) => code,
        }
    }

    pub fn into_phase2_reason_code(self) -> UnwindReasonCode {
        match self {
            UnwindError::PersonalityFailure(code) => code,
            _ => UnwindReasonCode::FATAL_PHASE2_ERROR,
        }
    }
}

pub struct UnwindContext<'a> {
    frame: Option<&'a Frame>,
    ctx: &'a mut arch::unwinding::Context,
    signal: bool,
}

// ===================================
// Now comes all the nasty FFI stuff
// ===================================

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

impl fmt::Debug for UnwindReasonCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UnwindReasonCode(")?;
        match *self {
            Self::NO_REASON => write!(f, "NO_REASON")?,
            Self::FOREIGN_EXCEPTION_CAUGHT => write!(f, "FOREIGN_EXCEPTION_CAUGHT")?,
            Self::FATAL_PHASE2_ERROR => write!(f, "FATAL_PHASE2_ERROR")?,
            Self::FATAL_PHASE1_ERROR => write!(f, "FATAL_PHASE1_ERROR")?,
            Self::NORMAL_STOP => write!(f, "NORMAL_STOP")?,
            Self::END_OF_STACK => write!(f, "END_OF_STACK")?,
            Self::HANDLER_FOUND => write!(f, "HANDLER_FOUND")?,
            Self::INSTALL_CONTEXT => write!(f, "INSTALL_CONTEXT")?,
            Self::CONTINUE_UNWIND => write!(f, "CONTINUE_UNWIND")?,
            _ => write!(f, "<invalid>")?,
        }
        write!(f, ")")
    }
}

bitflags! {
    #[repr(transparent)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct UnwindAction: c_int {
        const SEARCH_PHASE = 1;
        const CLEANUP_PHASE = 2;
        const HANDLER_FRAME = 4;
        const FORCE_UNWIND = 8;
        const END_OF_STACK = 16;
    }
}

#[repr(C)]
pub struct UnwindException {
    pub exception_class: u64,
    pub exception_cleanup: Option<UnwindExceptionCleanupFn>,
    pub(crate) stop_fn: Option<*const c_void>,
    pub(crate) handler_cfa: usize,
}

impl UnwindException {
    #[must_use]
    pub fn new(exception_class: u64, exception_cleanup: Option<UnwindExceptionCleanupFn>) -> Self {
        Self {
            exception_class,
            exception_cleanup,
            stop_fn: None,
            handler_cfa: 0,
        }
    }
}

pub type UnwindExceptionCleanupFn = unsafe extern "C" fn(UnwindReasonCode, *mut UnwindException);

pub type PersonalityRoutine = unsafe extern "C" fn(
    c_int,
    UnwindAction,
    u64,
    *mut UnwindException,
    &mut UnwindContext<'_>,
) -> UnwindReasonCode;

pub type UnwindTraceFn =
    extern "C" fn(ctx: &UnwindContext<'_>, arg: *mut c_void) -> UnwindReasonCode;

#[inline(never)]
#[no_mangle]
pub unsafe extern "C-unwind" fn _Unwind_RaiseException(
    exception: *mut UnwindException,
) -> UnwindReasonCode {
    with_context(|saved_ctx| {
        let mut ctx = saved_ctx.clone();

        let signal = match raise_exception_phase1(exception, &mut ctx) {
            Ok(signal) => signal,
            Err(err) => return err.into_phase1_reason_code(),
        };

        // Disambiguate normal frame and signal frame.
        let handler_cfa = ctx[arch::unwinding::SP] - usize::from(signal);

        unsafe {
            (*exception).stop_fn = None;
            (*exception).handler_cfa = handler_cfa;
        }

        if let Err(err) = raise_exception_phase2(exception, saved_ctx, handler_cfa) {
            return err.into_phase2_reason_code();
        };

        arch::unwinding::restore_context(saved_ctx);
    })
}

fn raise_exception_phase1(
    exception: *mut UnwindException,
    ctx: &mut arch::unwinding::Context,
) -> Result<bool, UnwindError> {
    let mut signal = false;

    loop {
        if let Some(frame) =
            Frame::from_context(ctx, signal).map_err(UnwindError::ConstructFrame)?
        {
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
                        break Ok(signal);
                    }
                    code => return Err(UnwindError::PersonalityFailure(code)),
                }
            }

            *ctx = frame.unwind(ctx).map_err(UnwindError::UnwindFrame)?;
            signal = frame.is_signal_trampoline();
        } else {
            return Err(UnwindError::EndOfStack);
        }
    }
}

fn raise_exception_phase2(
    exception: *mut UnwindException,
    ctx: &mut arch::unwinding::Context,
    handler_cfa: usize,
) -> Result<(), UnwindError> {
    let mut signal = false;

    loop {
        if let Some(frame) =
            Frame::from_context(ctx, signal).map_err(UnwindError::ConstructFrame)?
        {
            let frame_cfa = ctx[arch::unwinding::SP] - usize::from(signal);
            if let Some(personality) = frame.personality() {
                let code = unsafe {
                    personality(
                        1,
                        UnwindAction::CLEANUP_PHASE
                            | if frame_cfa == handler_cfa {
                                UnwindAction::HANDLER_FRAME
                            } else {
                                UnwindAction::empty()
                            },
                        (*exception).exception_class,
                        exception,
                        &mut UnwindContext {
                            frame: Some(&frame),
                            ctx,
                            signal,
                        },
                    )
                };

                match code {
                    UnwindReasonCode::CONTINUE_UNWIND => (),
                    UnwindReasonCode::INSTALL_CONTEXT => {
                        frame.adjust_stack_for_args(ctx);
                        return Ok(());
                    }
                    code => return Err(UnwindError::PersonalityFailure(code)),
                }
            }

            *ctx = frame.unwind(ctx).map_err(UnwindError::UnwindFrame)?;
            signal = frame.is_signal_trampoline();
        } else {
            return Err(UnwindError::EndOfStack);
        }
    }
}

/// Resume unwinding a given exception.
///
/// This function funnily enough is the only thing in the whole `unwinder` module that actually
/// needs the whole complicated C++/libunwind compatible ABI since it will be called by code-generated
/// landing pads when they want to resume the unwinding process
/// (to my knowledge unwind landing pads are generated for all `Drop` implementations as well as for every `catch_unwind`)
#[inline(never)]
#[no_mangle]
pub unsafe extern "C-unwind" fn _Unwind_Resume(exception: *mut UnwindException) -> ! {
    with_context(|ctx| {
        match unsafe { (*exception).stop_fn } {
            None => {
                let handler_cfa = unsafe { (*exception).handler_cfa };
                if let Err(_err) = raise_exception_phase2(exception, ctx, handler_cfa) {
                    arch::abort_internal(1);
                }
            }
            Some(_stop) => {
                arch::abort_internal(1);
            }
        }

        unsafe { arch::unwinding::restore_context(ctx) }
    })
}

// #[inline(never)]
// #[no_mangle]
// pub unsafe extern "C-unwind" fn _Unwind_ForcedUnwind(
//     exception: *mut UnwindException,
//     stop: UnwindStopFn,
//     stop_arg: *mut c_void,
// ) -> UnwindReasonCode {
//     with_context(|ctx| {})
// }

#[no_mangle]
pub extern "C" fn _Unwind_GetLanguageSpecificData(unwind_ctx: &UnwindContext<'_>) -> *mut c_void {
    unwind_ctx
        .frame
        .map_or(ptr::null_mut(), |f| f.lsda() as *mut c_void)
}

#[no_mangle]
pub extern "C" fn _Unwind_GetRegionStart(unwind_ctx: &UnwindContext<'_>) -> usize {
    unwind_ctx.frame.map_or(0, Frame::initial_address)
}

#[no_mangle]
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
pub extern "C" fn _Unwind_SetGR(unwind_ctx: &mut UnwindContext<'_>, index: c_int, value: usize) {
    unwind_ctx.ctx[Register(index as u16)] = value;
}

#[no_mangle]
pub extern "C" fn _Unwind_SetIP(unwind_ctx: &mut UnwindContext<'_>, value: usize) {
    unwind_ctx.ctx[arch::unwinding::RA] = value;
}

#[no_mangle]
pub extern "C" fn _Unwind_GetIPInfo(
    unwind_ctx: &UnwindContext<'_>,
    ip_before_insn: &mut c_int,
) -> usize {
    *ip_before_insn = i32::from(unwind_ctx.signal);
    unwind_ctx.ctx[arch::unwinding::RA]
}

#[no_mangle]
pub extern "C" fn _Unwind_GetTextRelBase(unwind_ctx: &UnwindContext<'_>) -> usize {
    unwind_ctx.frame.map_or(0, |f| {
        usize::try_from(f.bases().eh_frame.text.unwrap()).unwrap()
    })
}

#[no_mangle]
pub extern "C" fn _Unwind_GetDataRelBase(unwind_ctx: &UnwindContext<'_>) -> usize {
    unwind_ctx.frame.map_or(0, |f| {
        usize::try_from(f.bases().eh_frame.data.unwrap()).unwrap()
    })
}

#[no_mangle]
pub unsafe extern "C" fn _Unwind_DeleteException(exception: *mut UnwindException) {
    if let Some(cleanup) = unsafe { (*exception).exception_cleanup } {
        unsafe { cleanup(UnwindReasonCode::FOREIGN_EXCEPTION_CAUGHT, exception) };
    }
}

// #[inline(never)]
// #[no_mangle]
// pub extern "C-unwind" fn _Unwind_Backtrace(
//     trace: UnwindTraceFn,
//     trace_argument: *mut c_void,
// ) -> UnwindReasonCode {
//     with_context(|ctx| {})
// }
