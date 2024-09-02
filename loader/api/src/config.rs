use cfg_if::cfg_if;

const CFG_MAGIC: u32 = u32::from_le_bytes(*b"lcfg");

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[non_exhaustive]
#[repr(C)]
pub struct LoaderConfig {
    magic: u32,
    /// The size of the stack that the loader should allocate for the kernel (in pages).
    ///
    /// The loader starts the kernel with a valid stack pointer. This setting defines
    /// the stack size that the loader should allocate and map.
    ///
    /// The stack is created with an additional guard page, so a stack overflow will lead to
    /// a page fault.
    pub kernel_stack_size_pages: u32,
    /// The size of the kernel heap in pages.
    ///
    /// If specified the loader will create and map a heap for the kernel.
    pub kernel_heap_size_pages: Option<u32>,
    /// The virtual memory mode to use when setting up the page tables.
    pub memory_mode: MemoryMode,
}

impl LoaderConfig {
    /// Creates a new default configuration with the following values:
    ///
    /// - `kernel_stack_size_pages`: 20
    /// - `kernel_heap_size_pages`: None
    /// - `memory_mode`: The default memory mode for the target architecture (Sv39 for Risc-V).
    #[must_use]
    pub const fn new_default() -> Self {
        Self {
            magic: CFG_MAGIC,
            kernel_stack_size_pages: 20,
            kernel_heap_size_pages: None,
            memory_mode: MemoryMode::new_default(),
        }
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

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[non_exhaustive]
pub enum MemoryMode {
    Riscv64Sv39,
    Riscv64Sv48,
    Riscv64Sv57,
}

impl MemoryMode {
    pub const fn new_default() -> Self {
        cfg_if! {
            if #[cfg(target_arch = "riscv64")] {
                Self::Riscv64Sv39
            } else {
                panic!("Unsupported target architecture");
            }
        }
    }
}

impl Default for MemoryMode {
    fn default() -> Self {
        Self::new_default()
    }
}
