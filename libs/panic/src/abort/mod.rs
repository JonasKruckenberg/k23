use crate::arch;
use alloc::boxed::Box;
use core::any::Any;
use core::panic::PanicPayload;

pub mod panic_count;

/// Mirroring std, this is an unmangled function on which to slap
/// yer breakpoints for backtracing panics.
#[inline(never)]
#[no_mangle]
pub fn rust_panic(_: &mut dyn PanicPayload) -> ! {
    arch::abort();
}

pub unsafe fn r#try<R, F: FnOnce() -> R>(f: F) -> Result<R, Box<dyn Any + Send>> {
    Ok(f())
}
