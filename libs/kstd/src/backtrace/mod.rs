use crate::unwinding;
use core::ffi::c_void;
use core::ptr::addr_of_mut;
use core::{fmt, mem};

pub enum Frame<'a> {
    Raw(&'a unwinding::UnwindContext<'a>),
    Cloned {
        ip: *mut c_void,
        sp: *mut c_void,
        symbol_address: *mut c_void,
    },
}

impl<'a> Frame<'a> {
    /// Returns the current instruction pointer of this frame.
    pub fn ip(&self) -> *mut c_void {
        match self {
            Frame::Raw(ctx) => unwinding::_Unwind_GetIP(ctx) as *mut c_void,
            Frame::Cloned { ip, .. } => *ip,
        }
    }

    /// Returns the current stack pointer of this frame.
    pub fn sp(&self) -> *mut c_void {
        match self {
            Frame::Raw(ctx) => unwinding::_Unwind_GetCFA(ctx) as *mut c_void,
            Frame::Cloned { sp, .. } => *sp,
        }
    }

    /// Returns the starting symbol address of the frame of this function.
    pub fn symbol_address(&self) -> *mut c_void {
        if let Frame::Cloned { symbol_address, .. } = *self {
            return symbol_address;
        }

        unwinding::_Unwind_FindEnclosingFunction(self.ip())
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

pub unsafe fn trace_unsynchronized<F: FnMut(&Frame) -> bool>(mut cb: F) {
    trace_imp(&mut cb)
}

fn trace_imp(mut cb: &mut dyn FnMut(&Frame) -> bool) {
    unwinding::_Unwind_Backtrace(trace_fn, addr_of_mut!(cb).cast());

    extern "C" fn trace_fn(
        ctx: &unwinding::UnwindContext,
        arg: *mut c_void,
    ) -> unwinding::UnwindReasonCode {
        let cb = unsafe { &mut *arg.cast::<&mut dyn FnMut(&Frame) -> bool>() };

        let guard = DropGuard;
        let keep_going = cb(&Frame::Raw(ctx));
        mem::forget(guard);

        if keep_going {
            unwinding::UnwindReasonCode::NO_REASON
        } else {
            unwinding::UnwindReasonCode::FATAL_PHASE1_ERROR
        }
    }
}

struct DropGuard;

impl Drop for DropGuard {
    fn drop(&mut self) {
        panic!("cannot panic during the backtrace function");
    }
}
