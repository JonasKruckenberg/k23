// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ops::Range;

use riscv::satp;
use riscv::sbi::rfence::{sfence_vma, sfence_vma_asid};

use crate::{
    AddressRangeExt, Arch, GIB, KIB, MIB, MemoryAttributes, MemoryMode, MemoryModeBuilder,
    PhysicalAddress, TIB, VirtualAddress, WriteOrExecute,
};

#[cfg(target_pointer_width = "64")]
#[expect(clippy::identity_op, reason = "formatting")]
pub const RISCV64_SV39: MemoryMode = MemoryModeBuilder::<Riscv64, _, _>::new("RISC-V Sv39")
    .with_physmap(VirtualAddress::new(0xffffffc000000000))
    .with_level(1 * GIB, 512, true)
    .with_level(2 * MIB, 512, true)
    .with_level(4 * KIB, 512, true)
    .finish();

#[cfg(target_pointer_width = "64")]
#[expect(clippy::identity_op, reason = "formatting")]
pub const RISCV64_SV48: MemoryMode = MemoryModeBuilder::<Riscv64, _, _>::new("RISC-V Sv48")
    .with_physmap(VirtualAddress::new(0xffff800000000000))
    .with_level(512 * GIB, 512, true)
    .with_level(1 * GIB, 512, true)
    .with_level(2 * MIB, 512, true)
    .with_level(4 * KIB, 512, true)
    .finish();

#[cfg(target_pointer_width = "64")]
#[expect(clippy::identity_op, reason = "formatting")]
pub const RISCV64_SV57: MemoryMode = MemoryModeBuilder::<Riscv64, _, _>::new("RISC-V Sv57")
    .with_physmap(VirtualAddress::new(0xff00000000000000))
    .with_level(256 * TIB, 512, true)
    .with_level(512 * GIB, 512, true)
    .with_level(1 * GIB, 512, true)
    .with_level(2 * MIB, 512, true)
    .with_level(4 * KIB, 512, true)
    .finish();

pub struct Riscv64 {
    asid: u16,
    mode: satp::Mode,
}

impl Riscv64 {
    pub const fn new(asid: u16, mode: satp::Mode) -> Self {
        // TODO determine max bits for satp asid field and initialize ASID allocator?

        Self { asid, mode }
    }
}

impl Arch for Riscv64 {
    type PageTableEntry = PageTableEntry;

    fn memory_mode(&self) -> &'static MemoryMode {
        match self.mode {
            satp::Mode::Bare => unreachable!(),
            satp::Mode::Sv39 => const { &RISCV64_SV39 },
            satp::Mode::Sv48 => const { &RISCV64_SV48 },
            satp::Mode::Sv57 => const { &RISCV64_SV57 },
            satp::Mode::Sv64 => unimplemented!(),
        }
    }

    fn active_table(&self) -> Option<PhysicalAddress> {
        let satp = satp::read();

        debug_assert_eq!(satp.asid(), self.asid);
        debug_assert_eq!(satp.mode(), self.mode);

        let address = PhysicalAddress::new(satp.ppn() << 12);

        if address.get() > 0 {
            Some(address)
        } else {
            None
        }
    }

    unsafe fn set_active_table(&self, addr: PhysicalAddress) {
        let ppn = addr.get() >> 12_i32;

        // Safety: ensured by the caller.
        unsafe {
            satp::set(self.mode, self.asid, ppn);
        }
    }

    fn fence(&self, address_range: Range<VirtualAddress>) {
        sfence_vma_asid(
            0,
            usize::MAX,
            address_range.start.get(),
            address_range.len(),
            self.asid,
        )
        .unwrap();
    }

    fn fence_all(&self) {
        sfence_vma(0, usize::MAX, 0, usize::MAX).unwrap();
    }
}

mycelium_bitfield::bitfield! {
    pub struct PageTableEntry<usize> {
        /// TODO explain
        const VALID: bool;
        /// TODO explain
        const READ: bool;
        /// TODO explain
        const WRITE: bool;
        /// TODO explain
        const EXECUTE: bool;
        /// TODO explain
        const USER: bool;
        /// TODO explain
        const GLOBAL: bool;
        /// TODO explain
        const ACCESSED: bool;
        /// TODO explain
        const DIRTY: bool;
        /// Available for use by the kernel.
        const SOFTWARE_USE = 2;
        /// The physical address. This will either point to another page table or
        /// to an aligned block of physical memory.
        const ADDRESS = 44;
        // Reserved, must be set to zero
        const _RESERVED = 7;
        // TODO explain
        const PBMT: MemoryType;
        /// Indicates the PTE is part of a larger mapping with a naturally aligned power-of-2
        /// granularity. The only supported alignment at the moment is 64KiB.
        ///
        /// The motivation is that the entry can be cached in a TLB as one or more
        /// entries representing the contiguous region as if it were a single (large) page covered
        /// by a single translation. This compaction can help relieve TLB pressure in some
        /// scenarios.
        const NAPOT = 1;
    }
}

mycelium_bitfield::enum_from_bits! {
    // TODO explain
    #[derive(Debug)]
    pub enum MemoryType<u8> {
        // (default)
        None = 0b00,
        /// Non-cacheable, idempotent, weakly-ordered (RVWMO), main memory
        NonCacheable = 0b01,
        /// Non-cacheable, non-idempotent, strongly-ordered (I/O ordering), I/O
        NonCacheableIO = 0b10,
    }
}

impl super::PageTableEntry for PageTableEntry {
    fn new_leaf(address: PhysicalAddress, attributes: MemoryAttributes) -> Self {
        Self::new()
            .with(Self::VALID, true)
            .with(Self::ADDRESS, address.get())
            .with(Self::READ, attributes.allows_read())
            .with(Self::WRITE, attributes.allows_write())
            .with(Self::EXECUTE, attributes.allows_execution())
    }

    fn new_table(address: PhysicalAddress) -> Self {
        Self::new()
            .with(Self::VALID, true)
            .with(Self::ADDRESS, address.get())
    }

    const VACANT: Self = Self::new();

    fn is_vacant(&self) -> bool {
        !self.get(Self::VALID)
    }

    fn is_leaf(&self) -> bool {
        self.get(Self::VALID)
            && (self.get(Self::READ) || (self.get(Self::WRITE) || self.get(Self::EXECUTE)))
    }

    fn is_table(&self) -> bool {
        self.get(Self::VALID)
            && !self.get(Self::READ)
            && !self.get(Self::WRITE)
            && !self.get(Self::EXECUTE)
    }

    fn address(&self) -> PhysicalAddress {
        PhysicalAddress::new(self.get(Self::ADDRESS))
    }

    fn attributes(&self) -> MemoryAttributes {
        let write_or_execute = match (self.get(Self::WRITE), self.get(Self::EXECUTE)) {
            (true, false) => WriteOrExecute::Write,
            (false, true) => WriteOrExecute::Execute,
            (false, false) => WriteOrExecute::Neither,
            (true, true) => panic!("invalid"),
        };

        MemoryAttributes::new()
            .with(MemoryAttributes::READ, self.get(Self::READ))
            .with(MemoryAttributes::WRITE_OR_EXECUTE, write_or_execute)
    }
}
