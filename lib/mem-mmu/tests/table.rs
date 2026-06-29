// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::alloc::Layout;

use mem_core::arch::{Arch, PageTableEntry};
use mem_core::{FrameAllocator, MemoryAttributes, PhysicalAddress, Size4KiB};
use mem_mmu::Table;
use mem_testkit::{for_arch, Machine, MachineBuilder};
use proptest::prelude::*;

for_arch!(A in [
    Riscv64Sv39,
    #[cfg(not(miri))]
    Riscv64Sv48,
    #[cfg(not(miri))]
    Riscv64Sv57,
] {
    proptest! {
        /// Regression test for [`Table::is_empty`] (review Blocker: `|=` should be `&=`).
        ///
        /// `is_empty` must return `true` exactly when every entry is vacant. The buggy
        /// `|=` accumulation makes it unconditionally report `true`.
        #[test]
        fn is_empty_iff_all_entries_vacant(
            occupied in proptest::collection::hash_set(0u16..A::LEVELS[0].entries(), 0..32),
        ) {
            let machine: Machine<A> = MachineBuilder::new()
                .with_memory_regions([
                    Layout::from_size_align(0x20000, A::GRANULE_SIZE).unwrap()
                ])
                .finish();

            let (address_space, frame_allocator, physmap) =
                machine.bootstrap_address_space::<Size4KiB>(A::DEFAULT_PHYSMAP_BASE);
            let arch = address_space.arch();

            let mut table =
                Table::allocate(frame_allocator.by_ref(), &physmap, arch).unwrap();

            // Occupy the chosen entries with leaves. The leaf address is irrelevant —
            // `is_empty` only inspects each entry's vacancy.
            let leaf = <<A as Arch>::PageTableEntry as PageTableEntry>::new_leaf(
                PhysicalAddress::new(A::GRANULE_SIZE),
                MemoryAttributes::new().with(MemoryAttributes::READ, true),
            );
            for &index in &occupied {
                // Safety: `index` is in `0..A::LEVELS[0].entries()`, in-bounds for the root table.
                unsafe {
                    table.borrow_mut().set(index, leaf, &physmap, arch);
                }
            }

            prop_assert_eq!(
                table.borrow().is_empty(&physmap, arch),
                occupied.is_empty(),
            );
        }
    }
});
