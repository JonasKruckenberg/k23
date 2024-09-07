use crate::PanicHookInfo;
use alloc::boxed::Box;
use alloc::string::String;
use core::any::Any;
use core::mem;
use sync::RwLock;

pub(crate) static HOOK: RwLock<Hook> = RwLock::new(Hook::Default);

pub fn call(panic_hook_info: &PanicHookInfo) {
    match *HOOK.read() {
        Hook::Default => {
            default_hook(panic_hook_info);
        }
        Hook::Custom(ref hook) => hook(panic_hook_info),
    }
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

/// Sets the panic hook, replacing the previous one.
///
/// The panic hook is invoked when a thread panics, but before the panic runtime is invoked.
///
/// The default hook will attempt to print the panic message to the semihosting output.
///
/// # Safety
///
/// The caller has to ensure that this function is not called from a panicking thread.
pub unsafe fn set_hook(hook: Box<dyn Fn(&PanicHookInfo<'_>) + 'static + Sync + Send>) {
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
/// # Safety
///
/// The caller has to ensure that this function is not called from a panicking thread.
pub unsafe fn take_hook() -> Box<dyn Fn(&PanicHookInfo<'_>) + 'static + Sync + Send> {
    let mut hook = HOOK.write();
    let old_hook = mem::take(&mut *hook);
    drop(hook);

    old_hook.into_box()
}

/// Atomic combination of [`take_hook`] and [`set_hook`].
///
/// Use this to replace the panic handler with a new panic handler that does something and then executes the old handler.
///
/// # Safety
///
/// The caller has to ensure that this function is not called from a panicking thread.
pub unsafe fn update_hook<F>(hook_fn: F)
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

/// The default panic hook.
fn default_hook(info: &PanicHookInfo<'_>) {
    let location = info.location();
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
