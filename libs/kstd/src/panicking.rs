use crate::sync::Mutex;
use crate::{arch, declare_thread_local, heprintln};
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

    let message = info.message();
    let loc = info.location().unwrap(); // The current implementation always returns Some
    let context = match abort_reason {
        AbortReason::AlwaysAbort => "hart panicked, aborting.",
        AbortReason::PanicInHook => "hart panicked while processing panic. aborting.",
    };

    let hook = HOOK.lock();
    match *hook {
        Hook::Default => {
            default_hook(&PanicHookInfo::new(
                loc,
                Some(&format_args!("{}", message)),
                &context,
            ));
        }
        Hook::Custom(ref hook) => {
            hook(&PanicHookInfo::new(
                loc,
                Some(&format_args!("{}", message)),
                &context,
            ));
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
    arch::abort_internal(1);
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
    context: &'a str,
    message: Option<&'a fmt::Arguments<'a>>,
    location: &'a Location<'a>,
}

impl<'a> PanicHookInfo<'a> {
    #[inline]
    pub(crate) fn new(
        location: &'a Location<'a>,
        message: Option<&'a fmt::Arguments<'a>>,
        context: &'a str,
    ) -> Self {
        PanicHookInfo {
            context,
            location,
            message,
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
        formatter.write_str(":\n")?;
        if let Some(fmt_args) = self.message {
            fmt_args.fmt(formatter)?;
        }
        formatter.write_str("\n")?;
        formatter.write_str(self.context)?;
        Ok(())
    }
}
