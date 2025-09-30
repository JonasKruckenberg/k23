// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use cfg_if::cfg_if;

use crate::arch::{Arch, PageTableLevel, PageTableLevelsBuilder};
use crate::{MemoryAttributes, PhysicalAddress, VirtualAddress, WriteOrExecute};

const DEFAULT_ASID: u16 = 0;
const PAGE_SIZE: usize = 4096;

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

    fn new_empty() -> Self {
        Self::new()
    }

    fn is_vacant(&self) -> bool {
        !self.get(Self::VALID)
    }

    fn is_leaf(&self) -> bool {
        self.get(Self::VALID)
            && (self.get(Self::READ) || self.get(Self::WRITE) || self.get(Self::EXECUTE))
    }

    fn address(&self) -> PhysicalAddress {
        PhysicalAddress::new(self.get(Self::ADDRESS))
    }

    unsafe fn set_address(&mut self, address: PhysicalAddress) {
        self.set(Self::ADDRESS, address.get());
    }

    fn attributes(&self) -> MemoryAttributes {
        MemoryAttributes::new()
            .with(MemoryAttributes::READ, self.get(Self::READ))
            .with(
                MemoryAttributes::WRITE_OR_EXECUTE,
                match (self.get(Self::WRITE), self.get(Self::EXECUTE)) {
                    (true, false) => WriteOrExecute::Write,
                    (false, true) => WriteOrExecute::Execute,
                    (false, false) => WriteOrExecute::Neither,
                    (true, true) => panic!("invalid"),
                },
            )
    }

    unsafe fn set_attributes(&mut self, attributes: MemoryAttributes) {
        self.set(Self::READ, attributes.allows_read())
            .set(Self::WRITE, attributes.allows_write())
            .set(Self::EXECUTE, attributes.allows_execution());
    }
}

pub struct RiscV64Sv39 {
    asid: u16,
}

impl RiscV64Sv39 {
    pub const fn new() -> Self {
        Self { asid: DEFAULT_ASID }
    }
}

impl Arch for RiscV64Sv39 {
    const PAGE_SIZE: usize = PAGE_SIZE;
    const PAGE_TABLE_LEVELS: &'static [PageTableLevel] =
        &PageTableLevelsBuilder::with_page_size(PAGE_SIZE)
            .with_level("L0", 512, true)
            .with_level("L1", 512, true)
            .with_level("L2", 512, true)
            .finish();

    type PageTableEntry = PageTableEntry;

    fn phys_to_virt(phys: PhysicalAddress) -> VirtualAddress {
        let kernel_aspace_base: VirtualAddress = VirtualAddress::new(0xffffffc000000000);

        kernel_aspace_base.add(phys.get())
    }

    cfg_if! {
        if #[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))] {
            unsafe fn active_table(&mut self) -> PhysicalAddress {
                let satp = riscv::satp::read();
                debug_assert_eq!(satp.asid(), self.asid);

                let addr = PhysicalAddress::new(satp.ppn() << 12);
                debug_assert!(addr.get() != 0);

                addr
            }

            unsafe fn set_active_table(&mut self, addr: PhysicalAddress) {
                let ppn = addr.get() >> 12_i32;

                // Safety: register access
                unsafe {
                    riscv::satp::set(riscv::satp::Mode::Sv39, self.asid, ppn);
                }
            }
        } else {
            unsafe fn active_table(&mut self) -> PhysicalAddress {
                unimplemented!()
            }
            unsafe fn set_active_table(&mut self, _addr: PhysicalAddress) {
                unimplemented!()
            }
        }
    }
}
