/// The log level for the kernel
#[kconfig_declare::symbol({
    paths: ["loader.log-level", "log-level"],
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
#[kconfig_declare::symbol("loader.stack-size-pages")]
pub const STACK_SIZE_PAGES: usize = 128;

/// Whether to enable Kernel Address Space Layout Randomization (KASLR)
#[kconfig_declare::symbol("loader.enable-kaslr")]
pub const ENABLE_KASLR: bool = true;

// TODO: This should be configurable
#[allow(non_camel_case_types)]
pub type MEMORY_MODE = kmm::Riscv64Sv39;
pub const PAGE_SIZE: usize = <MEMORY_MODE as kmm::Mode>::PAGE_SIZE;
