thread_local::declare_thread_local! {
    static HARTID: usize;
}

pub fn set(hartid: usize) {
    HARTID.initialize_with(hartid, |_, _| {});
}

pub(crate) fn get() -> usize {
    HARTID.with(|hartid| *hartid)
}
