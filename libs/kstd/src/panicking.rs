use core::{any::Any, fmt, mem, panic::Location};

use crate::{
    heprintln,
    panic::PanicHookInfo,
    sync::{LocalThreadId, RwLock},
};
use alloc::{boxed::Box, string::String};
use core::panic::PanicPayload;
use lock_api::GetThreadId;

#[derive(Default)]
enum Hook {
    #[default]
    Default,
    Custom(Box<dyn Fn(&PanicHookInfo<'_>) + 'static + Sync + Send>),
}

impl Hook {
    #[inline]
    fn into_box(self) -> Box<dyn Fn(&PanicHookInfo<'_>) + 'static + Sync + Send> {
        match self {
            Hook::Default => Box::new(default_hook),
            Hook::Custom(hook) => hook,
        }
    }
}

static HOOK: RwLock<Hook> = RwLock::new(Hook::Default);

pub fn set_hook(hook: Box<dyn Fn(&PanicHookInfo<'_>) + 'static + Sync + Send>) {
    let new = Hook::Custom(hook);
    let mut hook = HOOK.write();
    let old = mem::replace(&mut *hook, new);
    drop(hook);
    // Only drop the old hook after releasing the lock to avoid deadlocking
    // if its destructor panics.
    drop(old);
}

pub fn take_hook() -> Box<dyn Fn(&PanicHookInfo<'_>) + 'static + Sync + Send> {
    let mut hook = HOOK.write();
    let old_hook = mem::take(&mut *hook);
    drop(hook);

    old_hook.into_box()
}

pub fn update_hook<F>(hook_fn: F)
where
    F: Fn(&(dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static), &PanicHookInfo<'_>)
        + Sync
        + Send
        + 'static,
{
    let mut hook = HOOK.write();
    let prev = mem::take(&mut *hook).into_box();
    *hook = Hook::Custom(Box::new(move |info| hook_fn(&prev, info)));
}

/// The default panic handler.
fn default_hook(info: &PanicHookInfo<'_>) {
    let thread_id = LocalThreadId::INIT.nonzero_thread_id();
    let location = info.location().unwrap();
    let msg = payload_as_str(info.payload());

    let _ = heprintln!("thread '{}' panicked at {}:\n{}", thread_id, location, msg);
}

/// Determines whether the current thread is unwinding because of panic.
#[inline]
pub fn panicking() -> bool {
    !panic_count::count_is_zero()
}

/// Entry point of panics from the core crate (`panic_impl` lang item).
// #[cfg(not(any(test, doctest)))]
#[panic_handler]
pub fn begin_panic_handler(info: &core::panic::PanicInfo<'_>) -> ! {
    use core::{any::Any, fmt};

    use alloc::string::String;

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
            // We do two allocations here, unfortunately. But (a) they're required with the current
            // scheme, and (b) we don't handle panic + OOM properly anyway (see comment in
            // begin_panic below).
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
    let loc = info.location().unwrap(); // The current implementation always returns Some
    if let Some(s) = msg.as_str() {
        rust_panic_with_hook(
            &mut StaticStrPayload(s),
            loc,
            info.can_unwind(),
            info.force_no_backtrace(),
        );
    } else {
        rust_panic_with_hook(
            &mut FormatStringPayload {
                inner: &msg,
                string: None,
            },
            loc,
            info.can_unwind(),
            info.force_no_backtrace(),
        );
    }
}

fn rust_panic_with_hook(
    payload: &mut dyn PanicPayload,
    location: &Location<'_>,
    can_unwind: bool,
    force_no_backtrace: bool,
) -> ! {
    let must_abort = panic_count::increase(true);

    // Check if we need to abort immediately.
    if let Some(must_abort) = must_abort {
        match must_abort {
            panic_count::MustAbort::PanicInHook => {
                // Don't try to format the message in this case, perhaps that is causing the
                // recursive panics. However if the message is just a string, no user-defined
                // code is involved in printing it, so that is risk-free.
                let message: &str = payload.as_str().unwrap_or_default();
                heprintln!(
                    "panicked at {}:\n{}\nthread panicked while processing panic. aborting.\n",
                    location,
                    message
                );
            }
            panic_count::MustAbort::AlwaysAbort => {
                // Unfortunately, this does not print a backtrace, because creating
                // a `Backtrace` will allocate, which we must avoid here.
                heprintln!("aborting due to panic at {}:\n{}\n", location, payload);
            }
        }
        crate::arch::abort_internal(1);
    }

    match *HOOK.read() {
        Hook::Default => {
            default_hook(&PanicHookInfo::new(
                location,
                payload.get(),
                can_unwind,
                force_no_backtrace,
            ));
        }
        Hook::Custom(ref hook) => hook(&PanicHookInfo::new(
            location,
            payload.get(),
            can_unwind,
            force_no_backtrace,
        )),
    }

    panic_count::finished_panic_hook();

    if !can_unwind {
        // If a thread panics while running destructors or tries to unwind
        // through a nounwind function (e.g. extern "C") then we cannot continue
        // unwinding and have to abort immediately.
        heprintln!("thread caused non-unwinding panic. aborting.\n");
        crate::arch::abort_internal(1);
    }

    rust_panic(payload)
}

pub fn rust_panic_without_hook(payload: Box<dyn Any + Send>) -> ! {
    panic_count::increase(false);

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

    rust_panic(&mut RewrapBox(payload))
}

/// Mirroring std, this is an unmangled function on which to slap
/// yer breakpoints for backtracing panics.
#[no_mangle]
#[inline(never)]
extern "Rust" fn rust_panic(msg: &mut dyn PanicPayload) -> ! {
    // TODO do the stack unwinding

    crate::arch::abort_internal(1);
}

mod panic_count {
    use core::{
        cell::Cell,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use crate::declare_thread_local;

    /// A reason for forcing an immediate abort on panic.
    #[derive(Debug)]
    pub enum MustAbort {
        AlwaysAbort,
        PanicInHook,
    }

    // Panic count for the current thread and whether a panic hook is currently
    // being executed..
    declare_thread_local! {
        static LOCAL_PANIC_COUNT: Cell<(usize, bool)> = const { Cell::new((0, false)) }
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
    pub fn get_count() -> usize {
        LOCAL_PANIC_COUNT.with(|c| c.get().0)
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

fn payload_as_str(payload: &dyn Any) -> &str {
    if let Some(&s) = payload.downcast_ref::<&'static str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.as_str()
    } else {
        "Box<dyn Any>"
    }
}

/// This function is called by the panic runtime if FFI code catches a Rust
/// panic but doesn't rethrow it. We don't support this case since it messes
/// with our panic count.
// #[cfg(not(test))]
pub(crate) fn rust_drop_panic() -> ! {
    heprintln!("Rust panics must be rethrown");
    crate::arch::abort_internal(1);
}

/// This function is called by the panic runtime if it catches an exception
/// object which does not correspond to a Rust panic.
// #[cfg(not(test))]
pub(crate) fn rust_foreign_exception() -> ! {
    heprintln!("Rust cannot catch foreign exceptions");
    crate::arch::abort_internal(1);
}
