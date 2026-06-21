// Copyright 2023-Present Jonas Kruckenberg
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
        /// Cacheability / ordering class of the region. See [`MemoryKind`].
        pub const KIND: MemoryKind;
    }
}

mycelium_bitfield::enum_from_bits! {
    /// Cacheability and ordering class of a mapped region.
    ///
    /// This distinguishes ordinary RAM from memory-mapped device registers so
    /// that architectures which encode it in the page table (aarch64 `MAIR` /
    /// `Device-nGnRnE`, x86_64 `PAT`, RISC-V `Svpbmt` `PBMT`) can do so.
    ///
    /// On the base RISC-V ISA cacheability is fixed per physical region by the
    /// platform's PMAs, so [`Device`](Self::Device) is honored regardless of the
    /// page table — wiring `PBMT` is future work and requires `Svpbmt`.
    #[derive(Debug, Eq, PartialEq)]
    pub enum MemoryKind<u8> {
        /// Ordinary cacheable, idempotent main memory. The default (all-zero) kind.
        Normal = 0b0,
        /// Non-cacheable, strongly-ordered memory-mapped I/O (device registers).
        Device = 0b1,
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
    ///
    /// `true` exactly when reading is permitted and neither writing nor
    /// execution is.
    pub fn is_read_only(&self) -> bool {
        self.allows_read() && !self.allows_write() && !self.allows_execution()
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

    /// Returns the [`MemoryKind`] (cacheability / ordering class) of the region.
    pub fn kind(&self) -> MemoryKind {
        self.get(Self::KIND)
    }

    /// Returns whether the region is [device memory](MemoryKind::Device).
    pub fn is_device(&self) -> bool {
        matches!(self.kind(), MemoryKind::Device)
    }
}

#[cfg(feature = "test_utils")]
impl proptest::arbitrary::Arbitrary for MemoryAttributes {
    type Parameters = ();
    type Strategy = proptest::strategy::BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        use proptest::prelude::*;

        // Generate only valid bit patterns: an arbitrary `u8` would allow the
        // `WRITE_OR_EXECUTE` pattern `0b11`, which has no `WriteOrExecute` variant
        // and panics in `get`.
        (any::<bool>(), 0u8..3)
            .prop_map(|(read, write_or_execute)| {
                let write_or_execute = match write_or_execute {
                    0 => WriteOrExecute::Neither,
                    1 => WriteOrExecute::Write,
                    2 => WriteOrExecute::Execute,
                    _ => unreachable!(),
                };

                MemoryAttributes::new()
                    .with(MemoryAttributes::READ, read)
                    .with(MemoryAttributes::WRITE_OR_EXECUTE, write_or_execute)
            })
            .boxed()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        /// `is_read_only` must be `true` exactly when reading is permitted and the
        /// `WRITE_OR_EXECUTE` field is `Neither` — it must not ignore that field.
        #[test]
        fn is_read_only_iff_read_and_not_write_or_execute(attrs: MemoryAttributes) {
            let expected = attrs.allows_read()
                && matches!(
                    attrs.get(MemoryAttributes::WRITE_OR_EXECUTE),
                    WriteOrExecute::Neither,
                );

            prop_assert_eq!(attrs.is_read_only(), expected);
        }
    }
}
