pub mod hio;

pub fn abort_internal(_code: i32) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}
