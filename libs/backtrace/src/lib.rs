#![no_std]

use core::ffi::c_void;
use core::ptr::addr_of_mut;
use core::{fmt, mem};

pub struct Frame<'a> {
    inner: FrameInner<'a>,
}

enum FrameInner<'a> {
    Raw(&'a unwind::UnwindContext<'a>),
    Cloned {
        ip: *mut c_void,
        sp: *mut c_void,
        symbol_address: *mut c_void,
    },
}

impl<'a> Clone for Frame<'a> {
    fn clone(&self) -> Self {
        Self {
            inner: FrameInner::Cloned {
                ip: self.ip(),
                sp: self.sp(),
                symbol_address: self.symbol_address(),
            },
        }
    }
}

impl<'a> Frame<'a> {
    /// Returns the current instruction pointer of this frame.
    #[must_use]
    pub fn ip(&self) -> *mut c_void {
        match self.inner {
            FrameInner::Raw(ctx) => unwind::_Unwind_GetIP(ctx) as *mut c_void,
            FrameInner::Cloned { ip, .. } => ip,
        }
    }

    /// Returns the current stack pointer of this frame.
    #[must_use]
    pub fn sp(&self) -> *mut c_void {
        match self.inner {
            FrameInner::Raw(ctx) => unwind::_Unwind_GetCFA(ctx) as *mut c_void,
            FrameInner::Cloned { sp, .. } => sp,
        }
    }

    /// Returns the starting symbol address of the frame of this function.
    #[must_use]
    pub fn symbol_address(&self) -> *mut c_void {
        if let FrameInner::Cloned { symbol_address, .. } = self.inner {
            return symbol_address;
        }

        unwind::_Unwind_FindEnclosingFunction(self.ip())
    }
}

impl<'a> fmt::Debug for Frame<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("ip", &self.ip())
            .field("sp", &self.sp())
            .field("symbol_address", &self.symbol_address())
            .finish()
    }
}

/// # Safety
///
/// This is unsafe as it is unsynchronized and not thread safe (TODO this assumption is copied from stdlib, verify)
pub unsafe fn trace_unsynchronized<F: FnMut(&Frame) -> bool>(mut cb: F) {
    trace_imp(&mut cb);
}

fn trace_imp(mut cb: &mut dyn FnMut(&Frame) -> bool) {
    extern "C" fn trace_fn(
        ctx: &unwind::UnwindContext,
        arg: *mut c_void,
    ) -> unwind::UnwindReasonCode {
        let cb = unsafe { &mut *arg.cast::<&mut dyn FnMut(&Frame) -> bool>() };

        let guard = DropGuard;
        let keep_going = cb(&Frame {
            inner: FrameInner::Raw(ctx),
        });
        mem::forget(guard);

        if keep_going {
            unwind::UnwindReasonCode::NO_REASON
        } else {
            unwind::UnwindReasonCode::FATAL_PHASE1_ERROR
        }
    }

    unwind::_Unwind_Backtrace(trace_fn, addr_of_mut!(cb).cast());
}

struct DropGuard;

impl Drop for DropGuard {
    fn drop(&mut self) {
        panic!("cannot panic during the backtrace function");
    }
}
