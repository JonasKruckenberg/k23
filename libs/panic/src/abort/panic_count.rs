#[inline]
pub fn increase(_run_panic_hook: bool) -> Option<()> {
    None
}

#[inline]
pub fn finished_panic_hook() {}

#[must_use]
#[inline]
pub fn count_is_zero() -> bool {
    true
}
