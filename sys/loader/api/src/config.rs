// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

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
}

impl LoaderConfig {
    /// Creates a new default configuration with the following values:
    ///
    /// - `kernel_stack_size_pages`: 20
    #[must_use]
    pub const fn new_default() -> Self {
        Self {
            magic: CFG_MAGIC,
            kernel_stack_size_pages: 20,
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
