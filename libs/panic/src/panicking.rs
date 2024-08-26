use crate::{arch, r#impl, PanicHookInfo};
use alloc::boxed::Box;
use alloc::string::String;
use core::any::Any;
use core::panic::{Location, PanicPayload};
use core::{fmt, mem};
use sync::RwLock;

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
    // Check if we need to abort immediately.
    #[cfg(feature = "unwind")]
    if let Some(must_abort) = r#impl::panic_count::increase(true) {
        match must_abort {
            r#impl::panic_count::MustAbort::PanicInHook => {
                // Don't try to format the message in this case, perhaps that is causing the
                // recursive panics. However, if the message is just a string, no user-defined
                // code is involved in printing it, so that is risk-free.
                let msg = payload_as_str(payload.get());
                log::error!(
                    "panicked at {location}:\n{msg}\nhart panicked while processing panic. aborting.\n",
                );
            } // panic_count::MustAbort::AlwaysAbort => {
              //     // Unfortunately, this does not print a backtrace, because creating
              //     // a `Backtrace` will allocate, which we must avoid here.
              //     heprintln!("aborting due to panic at {}:\n{}\n", location, payload);
              // }
        }

        crate::arch::abort();
    }

    match *HOOK.read() {
        Hook::Default => {
            default_hook(&PanicHookInfo::new(location, payload.get(), can_unwind));
        }
        Hook::Custom(ref hook) => hook(&PanicHookInfo::new(location, payload.get(), can_unwind)),
    }

    r#impl::panic_count::finished_panic_hook();

    if !can_unwind {
        // If a thread panics while running destructors or tries to unwind
        // through a nounwind function (e.g. extern "C") then we cannot continue
        // unwinding and have to abort immediately.
        log::error!("hart caused non-unwinding panic. aborting.\n");

        arch::abort();
    }

    r#impl::rust_panic(payload)
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

    r#impl::panic_count::increase(false);
    r#impl::rust_panic(&mut RewrapBox(payload))
}

#[derive(Default)]
pub(crate) enum Hook {
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

pub(crate) static HOOK: RwLock<Hook> = RwLock::new(Hook::Default);

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
        !crate::panicking(),
        "cannot modify the panic hook from a panicking hart"
    );

    let new = Hook::Custom(hook);
    let mut hook = HOOK.write();
    let old = mem::replace(&mut *hook, new);
    drop(hook);
    // Only drop the old hook after releasing the lock to avoid deadlocking
    // if its destructor panics.
    drop(old);
}

/// Unregisters the current panic hook and returns it, registering the default hook in its place.
///
/// # Panics
///
/// Panics if called from a panicking thread.
pub fn take_hook() -> Box<dyn Fn(&PanicHookInfo<'_>) + 'static + Sync + Send> {
    assert!(
        !crate::panicking(),
        "cannot modify the panic hook from a panicking hart"
    );

    let mut hook = HOOK.write();
    let old_hook = mem::take(&mut *hook);
    drop(hook);

    old_hook.into_box()
}

/// Atomic combination of [`take_hook`] and [`set_hook`].
///
/// Use this to replace the panic handler with a new panic handler that does something and then executes the old handler.
///
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
        !crate::panicking(),
        "cannot modify the panic hook from a panicking hart"
    );

    let mut hook = HOOK.write();
    let prev = mem::take(&mut *hook).into_box();
    *hook = Hook::Custom(Box::new(move |info| hook_fn(&prev, info)));
}

/// The default panic handler.
fn default_hook(info: &PanicHookInfo<'_>) {
    let location = info.location().unwrap();
    let msg = payload_as_str(info.payload());

    log::error!("hart panicked at {location}:\n{msg}");
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
