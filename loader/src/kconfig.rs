/// The log level for the kernel
pub const LOG_LEVEL: log::Level = log::Level::Trace;
/// The size of the stack in pages
pub const STACK_SIZE_PAGES: usize = 128;
#[allow(non_camel_case_types)]
pub type MEMORY_MODE = kmm::Riscv64Sv39;
pub const PAGE_SIZE: usize = <MEMORY_MODE as kmm::Mode>::PAGE_SIZE;
pub const KASLR: bool = false;
