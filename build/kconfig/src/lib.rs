#![no_std]

/// Configuration derived from the build
#[derive(Debug, Clone)]
pub struct KConfig {
    /// The kernel stack size in pages
    pub stack_size_pages: usize,
    /// The log level to enable, can be converted to a `log::Level` through `log::Level::from_usize`
    pub log_level: log::Level,
    /// The baud rate for the kernel UART debugging output
    pub uart_baud_rate: u32,
}
