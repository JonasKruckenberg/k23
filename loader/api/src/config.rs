const CFG_MAGIC: u32 = u32::from_le_bytes(*b"lcfg");

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[non_exhaustive]
#[repr(C)]
pub struct LoaderConfig {
    magic: u32,
}

impl LoaderConfig {
    /// Creates a new default configuration with the following values:
    ///
    /// - `kernel_stack_size_pages`: 20
    /// - `kernel_heap_size_pages`: None
    /// - `memory_mode`: The default memory mode for the target architecture (Sv39 for Risc-V).
    #[must_use]
    pub const fn new_default() -> Self {
        Self { magic: CFG_MAGIC }
    }

    /// Asserts that the configuration is valid.
    ///
    /// # Panics
    ///
    /// Panics if the configuration is invalid.
    pub fn assert_valid(&self) {
        assert_eq!(self.magic, CFG_MAGIC, "malformed loader config");
    }
}

impl Default for LoaderConfig {
    fn default() -> Self {
        Self::new_default()
    }
}
