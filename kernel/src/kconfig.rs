/// The log level for the kernel
#[kconfig_declare::symbol({
    paths: ["kernel.log-level", "log-level"],
    parse: parse_log_lvl
})]
pub const LOG_LEVEL: log::Level = log::Level::Trace;
const fn parse_log_lvl(s: &str) -> log::Level {
    match s.as_bytes() {
        b"error" => log::Level::Error,
        b"warn" => log::Level::Warn,
        b"info" => log::Level::Info,
        b"debug" => log::Level::Debug,
        b"trace" => log::Level::Trace,
        _ => panic!(),
    }
}

/// The size of the stack in pages
#[kconfig_declare::symbol("kernel.stack-size-pages")]
pub const STACK_SIZE_PAGES: u32 = 128;

/// The size of the trap handler stack in pages
#[kconfig_declare::symbol("kernel.trap-stack-size-pages")]
pub const TRAP_STACK_SIZE_PAGES: usize = 16;

/// The size of the kernel heap in pages
#[kconfig_declare::symbol("kernel.heap-size-pages")]
pub const HEAP_SIZE_PAGES: usize = 8192; // 32 MiB

// TODO: This should be configurable
#[allow(non_camel_case_types)]
pub type MEMORY_MODE = kmm::Riscv64Sv39;
pub const PAGE_SIZE: usize = <MEMORY_MODE as kmm::Mode>::PAGE_SIZE;
