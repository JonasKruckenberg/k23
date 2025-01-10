//! Signals based trap handling for WASM.
//!
//! For performance reasons, JIT compiled WASM code uses hardware faults for WASM traps. Explicit traps
//! get translated into jumps to invalid instructions (each invalid instructions text offset corresponds to a trap code)
//! while other traps such as accessing out of bounds memory are handled through the "regular" page fault
//! mechanism.
//! But at the end of the day, all traps manifest as signals and this module is concerned with catching them.
//!
//! The code might look intimidating, but is in reality quite simple. Here's the cliff notes:
//! - We register signal handlers for SIGSEV, SIGBUS, SIGILL, and SIGFPE with the handler function `trap_handler`.
//! - Inside `trap_handler` we first access `CallThreadState` which holds thread-local data required
//!     for trap handling (such as the VMContext ptr, and the jmp_buf for longjumping) it is also the
//!     place we store trap details into.
//! - Use `CallThreadState` and the signals PC & FP to determine the originating WASM module, and the associated trap code.
//! - Save the trap code, captured backtrace and other info in the `CallThreadState` TLS storage.
//! - `longjmp` back to the `catch_traps` function
//! - inside `catch_traps` read the unwind info from `CallThreadState` and turn it into a nice `Error`
//!
//! This placeholder implementation is pretty much copied from wasmtime, so if you want to see the proper
//! implementation see [their repo](https://github.com/bytecodealliance/wasmtime/blob/f406347a6e8af835f52bfb6868a64f01be2ee533/crates/wasmtime/src/runtime/vm/sys/unix/signals.rs)

#![expect(static_mut_refs, reason = "signal handlers are static mut")]

use core::ffi::c_void;
use core::mem::MaybeUninit;
use core::{mem, ptr};
use spin::once::Once;

static mut PREV_SIGSEGV: MaybeUninit<libc::sigaction> = MaybeUninit::uninit();
static mut PREV_SIGBUS: MaybeUninit<libc::sigaction> = MaybeUninit::uninit();
static mut PREV_SIGILL: MaybeUninit<libc::sigaction> = MaybeUninit::uninit();
static mut PREV_SIGFPE: MaybeUninit<libc::sigaction> = MaybeUninit::uninit();

pub unsafe fn ensure_signal_handlers_are_registered() {
    static SIGNAL_HANDLER: Once = Once::new();

    SIGNAL_HANDLER.call_once(|| {
        foreach_handler(|slot, signal| {
            let mut handler: libc::sigaction = mem::zeroed();
            // The flags here are relatively careful, and they are...
            //
            // SA_SIGINFO gives us access to information like the program
            // counter from where the fault happened.
            //
            // SA_ONSTACK allows us to handle signals on an alternate stack,
            // so that the handler can run in response to running out of
            // stack space on the main stack. Rust installs an alternate
            // stack with sigaltstack, so we rely on that.
            //
            // SA_NODEFER allows us to reenter the signal handler if we
            // crash while handling the signal, and fall through to the
            // Breakpad handler by testing handlingSegFault.
            handler.sa_flags = libc::SA_SIGINFO | libc::SA_NODEFER | libc::SA_ONSTACK;
            handler.sa_sigaction = trap_handler as usize;
            libc::sigemptyset(&mut handler.sa_mask);
            assert_eq!(
                libc::sigaction(signal, &handler, slot),
                0i32,
                "unable to install signal handler"
            );
        });
    });
}

unsafe fn foreach_handler(mut f: impl FnMut(*mut libc::sigaction, i32)) {
    // Allow handling OOB with signals on all architectures
    f(PREV_SIGSEGV.as_mut_ptr(), libc::SIGSEGV);

    // Handle `unreachable` instructions which execute `ud2` right now
    f(PREV_SIGILL.as_mut_ptr(), libc::SIGILL);

    // x86 and s390x use SIGFPE to report division by zero
    if cfg!(target_arch = "x86_64") || cfg!(target_arch = "s390x") {
        f(PREV_SIGFPE.as_mut_ptr(), libc::SIGFPE);
    }

    // Sometimes we need to handle SIGBUS too:
    // - On Darwin, guard page accesses are raised as SIGBUS.
    if cfg!(target_os = "macos") || cfg!(target_os = "freebsd") {
        f(PREV_SIGBUS.as_mut_ptr(), libc::SIGBUS);
    }

    // TODO(#1980): x86-32, if we support it, will also need a SIGFPE handler.
    // TODO(#1173): ARM32, if we support it, will also need a SIGBUS handler.
}

unsafe extern "C" fn trap_handler(
    signum: libc::c_int,
    siginfo: *mut libc::siginfo_t,
    context: *mut c_void,
) {
    let previous = match signum {
        libc::SIGSEGV => PREV_SIGSEGV.as_ptr(),
        libc::SIGBUS => PREV_SIGBUS.as_ptr(),
        libc::SIGFPE => PREV_SIGFPE.as_ptr(),
        libc::SIGILL => PREV_SIGILL.as_ptr(),
        _ => panic!("unknown signal: {signum}"),
    };

    // Safety: the block below has all sorts of unsafe code, accessing C-structs, reading registers etc.
    // all horrifically unsafe.
    let handled = (|| unsafe {
        let p = &crate::wasm::placeholder::trap_handling::TLS;

        // If no wasm code is executing, we don't handle this as a wasm
        // trap.
        let info = match p.get() {
            Some(info) => &*info,
            None => return false,
        };

        // If we hit an exception while handling a previous trap, that's
        // quite bad, so bail out and let the system handle this
        // recursive segfault.
        //
        // Otherwise flag ourselves as handling a trap, do the trap
        // handling, and reset our trap handling flag. Then we figure
        // out what to do based on the result of the trap handling.
        let faulting_addr = match signum {
            libc::SIGSEGV | libc::SIGBUS => Some((*siginfo).si_addr() as usize),
            _ => None,
        };

        let cx = &*(context.cast::<libc::ucontext_t>());
        let pc = usize::try_from((*cx.uc_mcontext).__ss.__pc).unwrap();
        let fp = usize::try_from((*cx.uc_mcontext).__ss.__fp).unwrap();

        // If this fault wasn't in wasm code, then it's not our problem
        let Some((code, text_offset)) = code_registry::lookup_code(pc) else {
            return false;
        };

        let Some(trap) = code.lookup_trap_code(text_offset) else {
            return false;
        };

        info.set_jit_trap(pc, fp, faulting_addr, trap);

        // On macOS this is a bit special, unfortunately. If we were to
        // `siglongjmp` out of the signal handler that notably does
        // *not* reset the sigaltstack state of our signal handler. This
        // seems to trick the kernel into thinking that the sigaltstack
        // is still in use upon delivery of the next signal, meaning
        // that the sigaltstack is not ever used again if we immediately
        // call `wasmtime_longjmp` here.
        //
        // Note that if we use `longjmp` instead of `siglongjmp` then
        // the problem is fixed. The problem with that, however, is that
        // `setjmp` is much slower than `sigsetjmp` due to the
        // preservation of the process's signal mask. The reason
        // `longjmp` appears to work is that it seems to call a function
        // (according to published macOS sources) called
        // `_sigunaltstack` which updates the kernel to say the
        // sigaltstack is no longer in use. We ideally want to call that
        // here but I don't think there's a stable way for us to call
        // that.
        //
        // Given all that, on macOS only, we do the next best thing. We
        // return from the signal handler after updating the register
        // context. This will cause control to return to our shim
        // function defined here which will perform the
        // `wasmtime_longjmp` (`siglongjmp`) for us. The reason this
        // works is that by returning from the signal handler we'll
        // trigger all the normal machinery for "the signal handler is
        // done running" which will clear the sigaltstack flag and allow
        // reusing it for the next signal. Then upon resuming in our custom
        // code we blow away the stack anyway with a longjmp.
        if cfg!(target_os = "macos") {
            unsafe extern "C" fn wasmtime_longjmp_shim(jmp_buf: *const u8) {
                crate::wasm::placeholder::setjmp::longjmp(jmp_buf.cast_mut().cast(), 1)
            }
            set_pc(
                context,
                wasmtime_longjmp_shim as usize,
                info.jmp_buf.as_ptr() as usize,
            );
            return true;
        }
        crate::wasm::placeholder::setjmp::longjmp(info.jmp_buf.as_ptr().cast(), 1)
    })();

    if handled {
        return;
    }

    let previous = *previous;
    if previous.sa_flags & libc::SA_SIGINFO != 0i32 {
        mem::transmute::<usize, extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void)>(
            previous.sa_sigaction,
        )(signum, siginfo, context);
    } else if previous.sa_sigaction == libc::SIG_DFL || previous.sa_sigaction == libc::SIG_IGN {
        libc::sigaction(signum, ptr::from_ref(&previous), ptr::null_mut());
    } else {
        mem::transmute::<usize, extern "C" fn(libc::c_int)>(previous.sa_sigaction)(signum);
    }
}

unsafe fn set_pc(cx: *mut c_void, pc: usize, arg1: usize) {
    let cx = &mut *(cx.cast::<libc::ucontext_t>());
    (*cx.uc_mcontext).__ss.__pc = pc as u64;
    (*cx.uc_mcontext).__ss.__x[0] = arg1 as u64;
}
