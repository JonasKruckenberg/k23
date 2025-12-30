// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mycelium_bitfield::bitfield! {
    /// Rules that dictate how a region of virtual memory may be accessed.
    ///
    /// # W^X
    ///
    /// In order to prevent malicious code execution as proactively as possible,
    /// [`AccessRules`] can either allow *writes* OR *execution* but never both. This is enforced
    /// through the [`WriteOrExecute`] enum field.
    #[derive(Default, PartialEq, Eq)]
    pub struct MemoryAttributes<u8> {
        /// If set, reading from the memory region is allowed.
        pub const READ: bool;
        /// Whether executing, or writing this memory region is allowed (or neither).
        pub const WRITE_OR_EXECUTE: WriteOrExecute;
    }
}

mycelium_bitfield::enum_from_bits! {
    /// Whether executing, or writing this memory region is allowed (or neither).
    ///
    /// This is an enum to enforce [`W^X`] at the type-level.
    ///
    /// [`W^X`]: AccessRules
    #[derive(Debug, Eq, PartialEq)]
    pub enum WriteOrExecute<u8> {
        /// Neither writing nor execution of the memory region is allowed.
        Neither = 0b00,
        /// Writing to the memory region is allowed.
        Write = 0b01,
        /// Executing code from the memory region is allowed.
        Execute = 0b10,
    }
}

impl MemoryAttributes {
    /// Returns whether these `MemoryAttributes` allow _only_ reading memory.
    pub const fn is_read_only(&self) -> bool {
        const READ_MASK: u8 = MemoryAttributes::READ.max_value();
        self.0 & READ_MASK == 1
    }

    /// Returns whether these `MemoryAttributes` allow reading from memory.
    pub fn allows_read(&self) -> bool {
        self.get(Self::READ)
    }

    /// Returns whether these `MemoryAttributes` allow writing to memory.
    pub fn allows_write(&self) -> bool {
        matches!(self.get(Self::WRITE_OR_EXECUTE), WriteOrExecute::Write)
    }

    /// Returns whether these `MemoryAttributes` allow executing instructions from memory.
    pub fn allows_execution(&self) -> bool {
        matches!(self.get(Self::WRITE_OR_EXECUTE), WriteOrExecute::Execute)
    }
}
