use crate::arch;
use crate::sync::LocalThreadId;
use crate::{heprintln, panic::PanicHookInfo, sync::RwLock};
use alloc::boxed::Box;
use alloc::string::String;
use core::any::Any;
use core::panic::{Location, PanicPayload};
use core::{fmt, mem};
use lock_api::GetThreadId;

/// Entry point for panics from the `core` crate.
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
        rust_panic_with_hook(&mut StaticStrPayload(s), loc, info.can_unwind());
    } else {
        rust_panic_with_hook(
            &mut FormatStringPayload {
                inner: &msg,
                string: None,
            },
            loc,
            info.can_unwind(),
        );
    }
}

/// The central function for dealing with panics.
///
/// This checks for recursive panics, invokes panic hooks, and finally dispatches the panic to
/// the runtime.
fn rust_panic_with_hook(
    payload: &mut dyn PanicPayload,
    location: &Location<'_>,
    can_unwind: bool,
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
            } // panic_count::MustAbort::AlwaysAbort => {
              //     // Unfortunately, this does not print a backtrace, because creating
              //     // a `Backtrace` will allocate, which we must avoid here.
              //     heprintln!("aborting due to panic at {}:\n{}\n", location, payload);
              // }
        }
        crate::arch::abort_internal(1);
    }

    match *HOOK.read() {
        Hook::Default => {
            default_hook(&PanicHookInfo::new(location, payload.get(), can_unwind));
        }
        Hook::Custom(ref hook) => hook(&PanicHookInfo::new(location, payload.get(), can_unwind)),
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

/// Passes the panic straight to the runtime, bypassing any configured
/// panic hooks. This is currently only used by `panic::resume_unwind`.
pub fn rust_panic_without_hook(payload: Box<dyn Any + Send>) -> ! {
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

/// Mirroring std, this is an unmangled function on which to slap
/// yer breakpoints for backtracing panics.
#[inline(never)]
#[no_mangle]
#[cfg(not(feature = "panic-unwind"))]
fn rust_panic(_: &mut dyn PanicPayload) -> ! {
    arch::abort_internal(1);
}

#[inline(never)]
#[no_mangle]
#[cfg(feature = "panic-unwind")]
extern "Rust" fn rust_panic(payload: &mut dyn PanicPayload) -> ! {
    let code = crate::unwinding::panic_begin(unsafe { Box::from_raw(payload.take_box()) });

    arch::abort_internal(code);
}

#[cfg(not(feature = "panic-unwind"))]
pub unsafe fn r#try<R, F: FnOnce() -> R>(f: F) -> Result<R, Box<dyn Any + Send>> {
    Ok(f())
}

#[cfg(feature = "panic-unwind")]
#[allow(clippy::items_after_statements)]
pub unsafe fn r#try<R, F: FnOnce() -> R>(f: F) -> Result<R, Box<dyn Any + Send>> {
    use core::{intrinsics, mem::ManuallyDrop, ptr::addr_of_mut};

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

    #[inline]
    #[rustc_nounwind] // `intrinsic::r#try` requires catch fn to be nounwind
    fn do_catch<F: FnOnce() -> R, R>(data: *mut u8, payload: *mut u8) {
        // SAFETY: this is the responsibility of the caller, see above.
        //
        // When `__rustc_panic_cleaner` is correctly implemented we can rely
        // on `obj` being the correct thing to pass to `data.p` (after wrapping
        // in `ManuallyDrop`).
        unsafe {
            let data = data.cast::<Data<F, R>>();
            let data = &mut (*data);
            let obj = cleanup(payload);
            data.p = ManuallyDrop::new(obj);
        }
    }

    #[cold]
    unsafe fn cleanup(payload: *mut u8) -> Box<dyn Any + Send + 'static> {
        // SAFETY: The whole unsafe block hinges on a correct implementation of
        // the panic handler `__rust_panic_cleanup`. As such we can only
        // assume it returns the correct thing for `Box::from_raw` to work
        // without undefined behavior.
        let obj = unsafe { crate::unwinding::panic_cleanup(payload) };
        panic_count::decrease();
        obj
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

/// Sets the panic hook, replacing the previous one.
///
/// The panic hook is invoked when a thread panics, but before the panic runtime is invoked.
///
/// The default hook will attempt to print the panic message to the semihosting output.
///
/// # Panics
///
/// Panics if called from a panicking thread.
pub fn set_hook(hook: Box<dyn Fn(&PanicHookInfo<'_>) + 'static + Sync + Send>) {
    assert!(
        !panicking(),
        "cannot modify the panic hook from a panicking thread"
    );

    let new = Hook::Custom(hook);
    let mut hook = HOOK.write();
    let old = mem::replace(&mut *hook, new);
    drop(hook);
    // Only drop the old hook after releasing the lock to avoid deadlocking
    // if its destructor panics.
    drop(old);
}

/// # Panics
///
/// Panics if called from a panicking thread.
pub fn take_hook() -> Box<dyn Fn(&PanicHookInfo<'_>) + 'static + Sync + Send> {
    assert!(
        !panicking(),
        "cannot modify the panic hook from a panicking thread"
    );

    let mut hook = HOOK.write();
    let old_hook = mem::take(&mut *hook);
    drop(hook);

    old_hook.into_box()
}

/// # Panics
///
/// Panics if called from a panicking thread.
pub fn update_hook<F>(hook_fn: F)
where
    F: Fn(&(dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static), &PanicHookInfo<'_>)
        + Sync
        + Send
        + 'static,
{
    assert!(
        !panicking(),
        "cannot modify the panic hook from a panicking thread"
    );

    let mut hook = HOOK.write();
    let prev = mem::take(&mut *hook).into_box();
    *hook = Hook::Custom(Box::new(move |info| hook_fn(&prev, info)));
}

/// The default panic handler.
fn default_hook(info: &PanicHookInfo<'_>) {
    let thread_id = LocalThreadId::INIT.nonzero_thread_id();
    let location = info.location().unwrap();
    let msg = payload_as_str(info.payload());

    heprintln!("thread '{}' panicked at {}:\n{}", thread_id, location, msg);

    #[cfg(feature = "panic-unwind")]
    unsafe {
        crate::bascktrace::trace_unsynchronized(|frame| {
            heprintln!("{:?}", frame);

            true
        })
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

/// Determines whether the current thread is unwinding because of panic.
#[inline]
pub fn panicking() -> bool {
    !panic_count::count_is_zero()
}

#[cfg(feature = "panic-unwind")]
mod panic_count {
    use crate::declare_thread_local;
    use core::{
        cell::Cell,
        sync::atomic::{AtomicUsize, Ordering},
    };

    /// A reason for forcing an immediate abort on panic.
    #[derive(Debug)]
    pub enum MustAbort {
        // AlwaysAbort,
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
    // #[must_use]
    // pub fn get_count() -> usize {
    //     LOCAL_PANIC_COUNT.with(|c| c.get().0)
    // }

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

#[cfg(not(feature = "panic-unwind"))]
pub mod panic_count {
    /// A reason for forcing an immediate abort on panic.
    #[derive(Debug)]
    pub enum MustAbort {
        // AlwaysAbort,
        PanicInHook,
    }

    #[inline]
    pub fn increase(run_panic_hook: bool) -> Option<MustAbort> {
        None
    }

    #[inline]
    pub fn finished_panic_hook() {}

    #[inline]
    pub fn decrease() {}

    #[inline]
    pub fn set_always_abort() {}

    // Disregards ALWAYS_ABORT_FLAG
    // #[inline]
    // #[must_use]
    // pub fn get_count() -> usize {
    //     0
    // }

    #[must_use]
    #[inline]
    pub fn count_is_zero() -> bool {
        true
    }
}
