use kstd::declare_thread_local;

pub type EntryFlags = vmm::EmulateEntryFlags;

declare_thread_local! {
    pub static HARTID: usize;
}
