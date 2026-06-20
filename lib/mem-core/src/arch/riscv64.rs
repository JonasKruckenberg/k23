// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::range::Range;

use human_bytes::{GIB, KIB, MIB, TIB};

use crate::arch::PageTableLevel;
use crate::{MemoryAttributes, PhysicalAddress, VirtualAddress, WriteOrExecute};

/// The number of usable bits in a `PhysicalAddress`. This may be used for address canonicalization.
#[cfg_attr(not(test), expect(unused, reason = "only used by tests"))]
const PHYSICAL_ADDRESS_BITS: usize = 56;

const PAGE_OFFSET_BITS: usize = 12;

pub struct Riscv64Sv39 {
    asid: u16,
}

impl Riscv64Sv39 {
    pub const fn new(asid: u16) -> Riscv64Sv39 {
        Riscv64Sv39 { asid }
    }

    pub const fn asid(&self) -> u16 {
        self.asid
    }
}

impl super::Arch for Riscv64Sv39 {
    type PageTableEntry = PageTableEntry;

    const DEFAULT_PHYSMAP_BASE: VirtualAddress = VirtualAddress::new(0xffffffc000000000);

    #[expect(clippy::identity_op, reason = "formatting")]
    const LEVELS: &'static [PageTableLevel] = &[
        PageTableLevel::new(1 * GIB, 512, true),
        PageTableLevel::new(2 * MIB, 512, true),
        PageTableLevel::new(4 * KIB, 512, true),
    ];

    fn active_table(&self) -> Option<PhysicalAddress> {
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                active_table(self.asid, riscv::satp::Mode::Sv39)
            } else {
                unimplemented!()
            }
        }
    }

    unsafe fn set_active_table(&self, address: PhysicalAddress) {
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                // Safety: we're accessing a control register here. The consequences of which
                // are explained to our caller and it is their responsibility to ensure this is safe.
                unsafe { set_active_table(self.asid, riscv::satp::Mode::Sv39, address) };
            } else {
                let _ = address;
                unimplemented!()
            }
        }
    }

    fn fence(&self, range: Range<VirtualAddress>) {
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                fence(self.asid, range);
            } else {
                let _ = range;
                unimplemented!()
            }
        }
    }

    fn fence_all(&self) {
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                fence_all();
            } else {
                unimplemented!()
            }
        }
    }
}

pub struct Riscv64Sv48 {
    asid: u16,
}

impl super::Arch for Riscv64Sv48 {
    type PageTableEntry = PageTableEntry;

    #[expect(clippy::identity_op, reason = "formatting")]
    const LEVELS: &'static [PageTableLevel] = &[
        PageTableLevel::new(512 * GIB, 512, true),
        PageTableLevel::new(1 * GIB, 512, true),
        PageTableLevel::new(2 * MIB, 512, true),
        PageTableLevel::new(4 * KIB, 512, true),
    ];

    const DEFAULT_PHYSMAP_BASE: VirtualAddress = VirtualAddress::new(0xffffffc000000000);

    fn active_table(&self) -> Option<PhysicalAddress> {
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                active_table(self.asid, riscv::satp::Mode::Sv48)
            } else {
                unimplemented!()
            }
        }
    }

    unsafe fn set_active_table(&self, address: PhysicalAddress) {
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                // Safety: we're accessing a control register here. The consequences of which
                // are explained to our caller and it is their responsibility to ensure this is safe.
                unsafe { set_active_table(self.asid, riscv::satp::Mode::Sv48, address) };
            } else {
                let _ = address;
                unimplemented!()
            }
        }
    }

    fn fence(&self, range: Range<VirtualAddress>) {
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                fence(self.asid, range);
            } else {
                let _ = range;
                unimplemented!()
            }
        }
    }

    fn fence_all(&self) {
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                fence_all();
            } else {
                unimplemented!()
            }
        }
    }
}

pub struct Riscv64Sv57 {
    asid: u16,
}

impl super::Arch for Riscv64Sv57 {
    type PageTableEntry = PageTableEntry;

    #[expect(clippy::identity_op, reason = "formatting")]
    const LEVELS: &'static [PageTableLevel] = &[
        PageTableLevel::new(256 * TIB, 512, true),
        PageTableLevel::new(512 * GIB, 512, true),
        PageTableLevel::new(1 * GIB, 512, true),
        PageTableLevel::new(2 * MIB, 512, true),
        PageTableLevel::new(4 * KIB, 512, true),
    ];

    const DEFAULT_PHYSMAP_BASE: VirtualAddress = VirtualAddress::new(0xffffffc000000000);

    fn active_table(&self) -> Option<PhysicalAddress> {
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                active_table(self.asid, riscv::satp::Mode::Sv57)
            } else {
                unimplemented!()
            }
        }
    }

    unsafe fn set_active_table(&self, address: PhysicalAddress) {
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                // Safety: we're accessing a control register here. The consequences of which
                // are explained to our caller and it is their responsibility to ensure this is safe.
                unsafe { set_active_table(self.asid, riscv::satp::Mode::Sv57, address) };
            } else {
                let _ = address;
                unimplemented!()
            }
        }
    }

    fn fence(&self, range: Range<VirtualAddress>) {
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                fence(self.asid, range);
            } else {
                let _ = range;
                unimplemented!()
            }
        }
    }

    fn fence_all(&self) {
        cfg_if::cfg_if! {
            if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                fence_all();
            } else {
                unimplemented!()
            }
        }
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

const _: () = {
    assert!(PageTableEntry::VALID.least_significant_index() == 0);
    assert!(PageTableEntry::VALID.most_significant_index() - 1 == 0);

    assert!(PageTableEntry::READ.least_significant_index() - 1 == 0);
    assert!(PageTableEntry::READ.most_significant_index() - 2 == 0);

    assert!(PageTableEntry::WRITE.least_significant_index() - 2 == 0);
    assert!(PageTableEntry::WRITE.most_significant_index() - 3 == 0);

    assert!(PageTableEntry::EXECUTE.least_significant_index() - 3 == 0);
    assert!(PageTableEntry::EXECUTE.most_significant_index() - 4 == 0);

    assert!(PageTableEntry::USER.least_significant_index() - 4 == 0);
    assert!(PageTableEntry::USER.most_significant_index() - 5 == 0);

    assert!(PageTableEntry::GLOBAL.least_significant_index() - 5 == 0);
    assert!(PageTableEntry::GLOBAL.most_significant_index() - 6 == 0);

    assert!(PageTableEntry::ACCESSED.least_significant_index() - 6 == 0);
    assert!(PageTableEntry::ACCESSED.most_significant_index() - 7 == 0);

    assert!(PageTableEntry::DIRTY.least_significant_index() - 7 == 0);
    assert!(PageTableEntry::DIRTY.most_significant_index() - 8 == 0);

    assert!(PageTableEntry::SOFTWARE_USE.least_significant_index() - 8 == 0);
    assert!(PageTableEntry::SOFTWARE_USE.most_significant_index() - 10 == 0);

    assert!(PageTableEntry::ADDRESS.least_significant_index() - 10 == 0);
    assert!(PageTableEntry::ADDRESS.most_significant_index() - 54 == 0);

    assert!(PageTableEntry::_RESERVED.least_significant_index() - 54 == 0);
    assert!(PageTableEntry::_RESERVED.most_significant_index() - 61 == 0);

    assert!(PageTableEntry::PBMT.least_significant_index() - 61 == 0);
    assert!(PageTableEntry::PBMT.most_significant_index() - 63 == 0);

    assert!(PageTableEntry::NAPOT.least_significant_index() - 63 == 0);
    assert!(PageTableEntry::NAPOT.most_significant_index() - 64 == 0);
};

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
        let address_raw = address.get() >> PAGE_OFFSET_BITS;

        debug_assert!(address_raw <= Self::ADDRESS.max_value());

        Self::new()
            .with(Self::VALID, true)
            .with(Self::ADDRESS, address_raw)
            .with(Self::READ, attributes.allows_read())
            .with(Self::WRITE, attributes.allows_write())
            .with(Self::EXECUTE, attributes.allows_execution())
    }

    fn new_table(address: PhysicalAddress) -> Self {
        let address_raw = address.get() >> PAGE_OFFSET_BITS;

        debug_assert!(address_raw <= Self::ADDRESS.max_value());

        Self::new()
            .with(Self::VALID, true)
            .with(Self::ADDRESS, address_raw)
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
        PhysicalAddress::new(self.get(Self::ADDRESS) << PAGE_OFFSET_BITS)
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

#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
fn active_table(asid: u16, mode: riscv::satp::Mode) -> Option<PhysicalAddress> {
    let satp = riscv::satp::read();

    debug_assert_eq!(satp.asid(), asid);
    debug_assert_eq!(satp.mode(), mode);

    let address = PhysicalAddress::new(satp.ppn() << 12);

    if address.get() > 0 {
        Some(address)
    } else {
        None
    }
}

#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
unsafe fn set_active_table(asid: u16, mode: riscv::satp::Mode, addr: PhysicalAddress) {
    let ppn = addr.get() >> 12_i32;

    // Safety: ensured by the caller.
    unsafe {
        riscv::satp::set(mode, asid, ppn);
    }
}

#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
fn fence(asid: u16, address_range: Range<VirtualAddress>) {
    use crate::AddressRangeExt;

    riscv::sbi::rfence::sfence_vma_asid(
        0,
        usize::MAX,
        address_range.start.get(),
        address_range.len(),
        asid,
    )
    .unwrap();
}

#[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))]
fn fence_all() {
    riscv::sbi::rfence::sfence_vma(0, usize::MAX, 0, usize::MAX).unwrap();
}

#[cfg(test)]
mod tests {
    use human_bytes::KIB;
    use proptest::{prop_assert_eq, proptest};

    use super::*;
    use crate::MemoryAttributes;
    use crate::arch::PageTableEntry;
    use crate::test_utils::proptest::{aligned_phys, phys};

    proptest! {
        #[test]
        #[cfg_attr(miri, ignore)]
        fn pte_new_leaf(address in aligned_phys(phys(0..1 << PHYSICAL_ADDRESS_BITS), 4*KIB)) {
            let pte = <super::PageTableEntry as PageTableEntry>::new_leaf(address, MemoryAttributes::new());

            prop_assert_eq!(pte.address(), address, "{}", pte);
        }
    }
}
