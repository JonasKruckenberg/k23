use crate::sync::Mutex;
use crate::{arch, declare_thread_local, heprintln};
use core::any::Any;
use core::cell::Cell;
use core::panic::{Location, PanicInfo};
use core::sync::atomic::{AtomicBool, Ordering};
use core::{fmt, mem};

static GLOBAL_PANICKING: AtomicBool = AtomicBool::new(false);

// Panic count for the current thread and whether a panic hook is currently
// being executed..
declare_thread_local! {
    static LOCAL_PANICKING: Cell<bool> = const { Cell::new(false) }
}

/// Determines whether the current hart is panicking.
#[inline]
pub fn panicking() -> bool {
    if GLOBAL_PANICKING.load(Ordering::Relaxed) == false {
        false
    } else {
        panicking_slow_path()
    }
}

// Slow path is in a separate function to reduce the amount of code
// inlined from `count_is_zero`.
#[inline(never)]
#[cold]
fn panicking_slow_path() -> bool {
    LOCAL_PANICKING.with(|c| c.get() == true)
}

enum AbortReason {
    AlwaysAbort,
    PanicInHook,
}

fn begin_panicking() -> AbortReason {
    GLOBAL_PANICKING.store(true, Ordering::Relaxed);

    let panicking = LOCAL_PANICKING.with(|c| c.replace(true));

    if panicking {
        AbortReason::PanicInHook
    } else {
        AbortReason::AlwaysAbort
    }
}

/// Entry point of Rust panics
#[cfg(not(any(test, doctest)))]
#[panic_handler]
fn default_panic_handler(info: &PanicInfo<'_>) -> ! {
    let abort_reason = begin_panicking();

    let loc = info.location().unwrap(); // The current implementation always returns Some
    let payload = match abort_reason {
        AbortReason::AlwaysAbort => "hart panicked, aborting.",
        AbortReason::PanicInHook => "hart panicked while processing panic. aborting.",
    };

    let hook = HOOK.lock();
    match *hook {
        Hook::Default => {
            default_hook(&PanicHookInfo::new(loc, &payload));
        }
        Hook::Custom(ref hook) => {
            hook(&PanicHookInfo::new(loc, &payload));
        }
    }
}

#[derive(Default)]
enum Hook {
    #[default]
    Default,
    Custom(fn(&PanicHookInfo<'_>) -> !),
}

// FIXME replace with RwLock
static HOOK: Mutex<Hook> = Mutex::new(Hook::Default);

fn default_hook(info: &PanicHookInfo<'_>) -> ! {
    heprintln!("{}", info);
    arch::abort_internal();
}

pub fn set_hook(hook: fn(&PanicHookInfo<'_>) -> !) {
    if LOCAL_PANICKING.with(|p| p.get()) {
        panic!("cannot modify the panic hook from a panicking thread");
    }

    let new = Hook::Custom(hook);
    let mut hook = HOOK.lock();
    let old = mem::replace(&mut *hook, new);
    drop(hook);
    // Only drop the old hook after releasing the lock to avoid deadlocking
    // if its destructor panics.
    drop(old);
}

pub struct PanicHookInfo<'a> {
    payload: &'a (dyn Any + Send),
    location: &'a Location<'a>,
}

impl<'a> PanicHookInfo<'a> {
    #[inline]
    pub(crate) fn new(location: &'a Location<'a>, payload: &'a (dyn Any + Send)) -> Self {
        PanicHookInfo { payload, location }
    }

    /// Returns the payload associated with the panic.
    ///
    /// This will commonly, but not always, be a `&'static str` or [`String`].
    pub fn payload(&self) -> &(dyn Any + Send) {
        self.payload
    }

    #[must_use]
    #[inline]
    pub fn payload_as_str(&self) -> Option<&str> {
        if let Some(s) = self.payload.downcast_ref::<&str>() {
            Some(s)
        } else {
            None
        }
    }

    #[must_use]
    #[inline]
    pub fn location(&self) -> Option<&Location<'_>> {
        // NOTE: If this is changed to sometimes return None,
        // deal with that case in std::panicking::default_hook and core::panicking::panic_fmt.
        Some(&self.location)
    }
}

impl fmt::Display for PanicHookInfo<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("panicked at ")?;
        self.location.fmt(formatter)?;
        if let Some(payload) = self.payload_as_str() {
            formatter.write_str(":\n")?;
            formatter.write_str(payload)?;
        }
        Ok(())
    }
}
