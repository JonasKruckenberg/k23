pub mod hio;

pub fn abort_internal() -> ! {
    unsafe {
        core::hint::unreachable_unchecked()
    }
}