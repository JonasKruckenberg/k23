/// The log level for the kernel
pub const LOG_LEVEL: log::Level = log::Level::Trace;
/// The size of the stack in pages
pub const STACK_SIZE_PAGES: u32 = 256;
/// The size of the trap handler stack in pages
pub const TRAP_STACK_SIZE_PAGES: usize = 16;
/// The size of the kernel heap in pages
pub const HEAP_SIZE_PAGES: u32 = 8192; // 32 MiB
