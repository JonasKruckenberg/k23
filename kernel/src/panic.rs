use crate::arch;
use crate::panic::panic_count::MustAbort;
use alloc::boxed::Box;
use alloc::string::String;
use core::any::Any;
use core::panic::{Location, PanicPayload, UnwindSafe};
use core::{fmt, mem};

/// Determines whether the current thread is unwinding because of panic.
#[inline]
pub fn panicking() -> bool {
    !panic_count::count_is_zero()
}

/// Invokes a closure, capturing the cause of an unwinding panic if one occurs.
///
/// # Errors
///
/// If the given closure panics, the panic cause will be returned in the Err variant.
pub fn catch_unwind<F, R>(f: F) -> Result<R, Box<dyn Any + Send + 'static>>
where
    F: FnOnce() -> R + UnwindSafe,
{
    unwind2::catch_unwind(f).inspect_err(|_| {
        panic_count::decrease() // decrease the panic count, since we caught it
    })
}

/// Triggers a panic, bypassing the panic hook.
pub fn resume_unwind(payload: Box<dyn Any + Send>) -> ! {
    struct RewrapBox(Box<dyn Any + Send>);

    unsafe impl PanicPayload for RewrapBox {
        fn take_box(&mut self) -> *mut (dyn Any + Send) {
            Box::into_raw(mem::replace(&mut self.0, Box::new(())))
        }

        fn get(&mut self) -> &(dyn Any + Send) {
            &*self.0
        }
    }

    impl fmt::Display for RewrapBox {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(payload_as_str(&self.0))
        }
    }

    panic_count::increase(false);
    rust_panic(&mut RewrapBox(payload))
}

/// Entry point for panics from the `core` crate.
#[panic_handler]
fn begin_panic_handler(info: &core::panic::PanicInfo<'_>) -> ! {
    struct FormatStringPayload<'a> {
        inner: &'a core::panic::PanicMessage<'a>,
        string: Option<String>,
    }

    impl FormatStringPayload<'_> {
        fn fill(&mut self) -> &mut String {
            let inner = self.inner;
            // Lazily, the first time this gets called, run the actual string formatting.
            self.string.get_or_insert_with(|| {
                let mut s = String::new();
                let mut fmt = fmt::Formatter::new(&mut s);
                let _err = fmt::Display::fmt(&inner, &mut fmt);
                s
            })
        }
    }

    unsafe impl PanicPayload for FormatStringPayload<'_> {
        fn take_box(&mut self) -> *mut (dyn Any + Send) {
            let contents = mem::take(self.fill());
            Box::into_raw(Box::new(contents))
        }

        fn get(&mut self) -> &(dyn Any + Send) {
            self.fill()
        }
    }

    impl fmt::Display for FormatStringPayload<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            if let Some(s) = &self.string {
                f.write_str(s)
            } else {
                fmt::Display::fmt(&self.inner, f)
            }
        }
    }

    struct StaticStrPayload(&'static str);

    unsafe impl PanicPayload for StaticStrPayload {
        fn take_box(&mut self) -> *mut (dyn Any + Send) {
            Box::into_raw(Box::new(self.0))
        }

        fn get(&mut self) -> &(dyn Any + Send) {
            &self.0
        }

        fn as_str(&mut self) -> Option<&str> {
            Some(self.0)
        }
    }

    impl fmt::Display for StaticStrPayload {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }

    let msg = info.message();
    let loc = info.location().unwrap(); // Currently, this is always `Some`.
    if let Some(s) = msg.as_str() {
        panic_handler(&mut StaticStrPayload(s), loc, info.can_unwind())
    } else {
        panic_handler(
            &mut FormatStringPayload {
                inner: &msg,
                string: None,
            },
            loc,
            info.can_unwind(),
        )
    }
}

fn panic_handler(payload: &mut dyn PanicPayload, loc: &Location<'_>, can_unwind: bool) -> ! {
    backtrace::__rust_end_short_backtrace(|| {
        let msg = payload_as_str(payload.get());

        if let Some(must_abort) = panic_count::increase(true) {
            match must_abort {
                MustAbort::PanicInHook => {
                    log::error!("panicked at {loc}:\n{msg}\nhart panicked while processing panic. aborting.\n");
                }
            }

            arch::abort();
        }

        // TODO panic processing here
        log::error!("hart panicked at {loc}:\n{msg}");

        // Run thread-local destructors
        unsafe {
            thread_local::destructors::run();
        }

        panic_count::finished_panic_hook();

        if !can_unwind {
            // If a thread panics while running destructors or tries to unwind
            // through a nounwind function (e.g. extern "C") then we cannot continue
            // unwinding and have to abort immediately.
            log::error!("hart caused non-unwinding panic. aborting.\n");
            arch::abort();
        }

        rust_panic(payload)
    })
}

/// Mirroring std, this is an unmangled function on which to slap
/// yer breakpoints for backtracing panics.
#[inline(never)]
#[no_mangle]
fn rust_panic(payload: &mut dyn PanicPayload) -> ! {
    match unwind2::begin_panic(unsafe { Box::from_raw(payload.take_box()) }) {
        Ok(_) => arch::exit(0),
        Err(_) => arch::abort(),
    }
}

fn payload_as_str(payload: &dyn Any) -> &str {
    if let Some(&s) = payload.downcast_ref::<&'static str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.as_str()
    } else {
        "Box<dyn Any>"
    }
}

mod panic_count {
    use core::{
        cell::Cell,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use thread_local::thread_local;

    /// A reason for forcing an immediate abort on panic.
    #[derive(Debug)]
    pub enum MustAbort {
        // AlwaysAbort,
        PanicInHook,
    }

    // Panic count for the current thread and whether a panic hook is currently
    // being executed.
    thread_local! {
        static LOCAL_PANIC_COUNT: Cell<(usize, bool)> = Cell::new((0, false));
    }

    static GLOBAL_PANIC_COUNT: AtomicUsize = AtomicUsize::new(0);

    pub fn increase(run_panic_hook: bool) -> Option<MustAbort> {
        LOCAL_PANIC_COUNT.with(|c| {
            let (count, in_panic_hook) = c.get();
            if in_panic_hook {
                return Some(MustAbort::PanicInHook);
            }
            c.set((count + 1, run_panic_hook));
            None
        })
    }

    pub fn finished_panic_hook() {
        LOCAL_PANIC_COUNT.with(|c| {
            let (count, _) = c.get();
            c.set((count, false));
        });
    }

    pub fn decrease() {
        GLOBAL_PANIC_COUNT.fetch_sub(1, Ordering::Relaxed);
        LOCAL_PANIC_COUNT.with(|c| {
            let (count, _) = c.get();
            c.set((count - 1, false));
        });
    }

    // Disregards ALWAYS_ABORT_FLAG
    #[must_use]
    #[inline]
    pub fn count_is_zero() -> bool {
        if GLOBAL_PANIC_COUNT.load(Ordering::Relaxed) == 0 {
            // Fast path: if `GLOBAL_PANIC_COUNT` is zero, all threads
            // (including the current one) will have `LOCAL_PANIC_COUNT`
            // equal to zero, so TLS access can be avoided.
            //
            // In terms of performance, a relaxed atomic load is similar to a normal
            // aligned memory read (e.g., a mov instruction in x86), but with some
            // compiler optimization restrictions. On the other hand, a TLS access
            // might require calling a non-inlinable function (such as `__tls_get_addr`
            // when using the GD TLS model).
            true
        } else {
            is_zero_slow_path()
        }
    }

    // Slow path is in a separate function to reduce the amount of code
    // inlined from `count_is_zero`.
    #[inline(never)]
    #[cold]
    fn is_zero_slow_path() -> bool {
        LOCAL_PANIC_COUNT.with(|c| c.get().0 == 0)
    }
}
