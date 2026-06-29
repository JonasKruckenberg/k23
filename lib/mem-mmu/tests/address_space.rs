// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod tests {
    use std::alloc::Layout;
    use std::range::Range;

    use mem_core::arch::{Arch, MapsAt};
    use mem_core::{
        AddressRangeExt, FrameAllocator, MemoryAttributes, Size4KiB, VirtualAddress, WriteOrExecute,
    };
    use mem_mmu::Flush;
    use mem_testkit::{archtest, Machine, MachineBuilder};

    archtest!([
        Riscv64Sv39,
        #[cfg(not(miri))] Riscv64Sv48,
        #[cfg(not(miri))] Riscv64Sv57,
    ] {
        #[test]
        fn map<A: Arch + MapsAt<Size4KiB>>() {
            let machine: Machine<A> = MachineBuilder::new()
                .with_memory_regions([
                    Layout::from_size_align(0xA000, A::GRANULE_SIZE).unwrap()
                ])
                .finish();

            let (mut address_space, frame_allocator, physmap) = machine.bootstrap_address_space::<Size4KiB>(A::DEFAULT_PHYSMAP_BASE);

            let frame = frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();

            let page = Range::from_start_len(VirtualAddress::new(0x7000), A::GRANULE_SIZE);

            let mut flush = Flush::new();
            unsafe {
                address_space
                    .map_contiguous::<Size4KiB>(
                        page.clone(),
                        frame,
                        MemoryAttributes::new().with(MemoryAttributes::READ, true),
                        frame_allocator.by_ref(),
                        &physmap,
                        &mut flush,
                    )
                    .unwrap();
            }
            flush.flush(address_space.arch());

            // TODO should use proptest instead of hardcoded offsets
            let (phys, attrs, lvl) = address_space.lookup(page.start.add(42), &physmap).unwrap();

            assert_eq!(phys, frame.add(42));
            assert_eq!(attrs.allows_read(), true);
            assert_eq!(attrs.allows_write(), false);
            assert_eq!(attrs.allows_execution(), false);
            assert_eq!(lvl.page_size(), 4096);
        }

        #[test]
        fn remap<A: Arch + MapsAt<Size4KiB>>() {
            let machine: Machine<A> = MachineBuilder::new()
                .with_memory_regions([Layout::from_size_align(0xB000, A::GRANULE_SIZE).unwrap()])
                .finish();

            let (mut address_space, frame_allocator, physmap) = machine.bootstrap_address_space::<Size4KiB>(A::DEFAULT_PHYSMAP_BASE);

            let frame = frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();

            let page = Range::from_start_len(VirtualAddress::new(0x7000), A::GRANULE_SIZE);

            let mut flush = Flush::new();
            unsafe {
                address_space
                    .map_contiguous::<Size4KiB>(
                        page.clone(),
                        frame,
                        MemoryAttributes::new().with(MemoryAttributes::READ, true),
                        frame_allocator.by_ref(),
                        &physmap,
                        &mut flush,
                    )
                    .unwrap();
            }
            flush.flush(address_space.arch());

            // TODO should use proptest instead of hardcoded offsets
            let (phys, attrs, lvl) = address_space.lookup(page.start.add(42), &physmap).unwrap();

            assert_eq!(phys, frame.add(42));
            assert_eq!(attrs.allows_read(), true);
            assert_eq!(attrs.allows_write(), false);
            assert_eq!(attrs.allows_execution(), false);
            assert_eq!(lvl.page_size(), 4096);

            // ===== the actual remap part =====

            let new_frame = frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();

            let mut flush = Flush::new();
            unsafe {
                address_space.remap_contiguous::<Size4KiB>(page.clone(), new_frame, &physmap, &mut flush);
            }
            flush.flush(address_space.arch());

            // TODO should use proptest instead of hardcoded offsets
            let (phys, attrs, lvl) = address_space.lookup(page.start.add(42), &physmap).unwrap();

            assert_eq!(phys, new_frame.add(42));
            assert_eq!(attrs.allows_read(), true);
            assert_eq!(attrs.allows_write(), false);
            assert_eq!(attrs.allows_execution(), false);
            assert_eq!(lvl.page_size(), 4096);
        }

        #[test]
        fn set_attributes<A: Arch + MapsAt<Size4KiB>>() {
            let machine: Machine<A> = MachineBuilder::new()
                .with_memory_regions([Layout::from_size_align(0xB000, A::GRANULE_SIZE).unwrap()])
                .finish();

            let (mut address_space, frame_allocator, physmap) = machine.bootstrap_address_space::<Size4KiB>(A::DEFAULT_PHYSMAP_BASE);

            let frame = frame_allocator
                .allocate_contiguous(A::GRANULE_LAYOUT)
                .unwrap();

            let page = Range::from_start_len(VirtualAddress::new(0x7000), A::GRANULE_SIZE);

            let mut flush = Flush::new();
            unsafe {
                address_space
                    .map_contiguous::<Size4KiB>(
                        page.clone(),
                        frame,
                        MemoryAttributes::new().with(MemoryAttributes::READ, true),
                        frame_allocator.by_ref(),
                        &physmap,
                        &mut flush,
                    )
                    .unwrap();
            }
            flush.flush(address_space.arch());

            // TODO should use proptest instead of hardcoded offsets
            let (phys, attrs, lvl) = address_space.lookup(page.start.add(42), &physmap).unwrap();

            assert_eq!(phys, frame.add(42));
            assert_eq!(attrs.allows_read(), true);
            assert_eq!(attrs.allows_write(), false);
            assert_eq!(attrs.allows_execution(), false);
            assert_eq!(lvl.page_size(), 4096);

            // ===== the actual remap part =====

            let mut flush = Flush::new();
            unsafe {
                address_space.set_attributes::<Size4KiB>(
                    page.clone(),
                    MemoryAttributes::new()
                        .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Execute),
                    &physmap,
                    &mut flush,
                );
            }
            flush.flush(address_space.arch());

            // TODO should use proptest instead of hardcoded offsets
            let (phys, attrs, lvl) = address_space.lookup(page.start.add(42), &physmap).unwrap();

            assert_eq!(phys, frame.add(42));
            assert_eq!(attrs.allows_read(), false);
            assert_eq!(attrs.allows_write(), false);
            assert_eq!(attrs.allows_execution(), true);
            assert_eq!(lvl.page_size(), 4096);
        }
    });
}

mod proptests {
    use core::alloc::Layout;
    use core::range::Range;

    use mem_core::arch::Arch;
    use mem_core::{FrameAllocator, MemoryAttributes, Size4KiB, VirtualAddress};
    use mem_mmu::Flush;
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
            #![proptest_config(ProptestConfig::with_cases(if cfg!(miri) { 10 } else { 250 }))]

            /// Regression test for `map_contiguous` across a higher-level page-table
            /// boundary (review Blocker).
            ///
            /// A contiguous physical block mapped to a contiguous virtual range that
            /// straddles a leaf-table boundary must map every virtual page to the
            /// matching physical page, in order. This currently fails for two
            /// compounding reasons:
            ///  - `PageTableEntries::next` (utils.rs) overshoots the per-entry sub-range
            ///    when the range start is not aligned to the level's page size.
            ///  - `visit_mut` (table.rs) descends sibling subtables LIFO.
            #[test]
            fn map_contiguous_across_table_boundary(
                pages_before in 1usize..=8,
                pages_after in 1usize..=8,
                boundary_idx in 1usize..256,
            ) {
                let machine: Machine<A> = MachineBuilder::new()
                    .with_memory_regions([
                        Layout::from_size_align(0x40000, A::GRANULE_SIZE).unwrap()
                    ])
                    .finish();

                let (mut address_space, frame_allocator, physmap) =
                    machine.bootstrap_address_space::<Size4KiB>(A::DEFAULT_PHYSMAP_BASE);

                let granule = A::GRANULE_SIZE;
                let total_pages = pages_before + pages_after;

                // A single contiguous physical block backing the whole mapping.
                let phys = frame_allocator
                    .allocate_contiguous(
                        Layout::from_size_align(total_pages * granule, granule).unwrap(),
                    )
                    .unwrap();

                // A contiguous virtual range straddling a leaf-table boundary: the
                // page size of the level whose entries point at leaf tables is the
                // span of one leaf table, and thus the gap between two of them.
                let leaf_table_span = A::LEVELS[A::LEVELS.len() - 2].page_size();
                let boundary = VirtualAddress::new(boundary_idx * leaf_table_span);
                let virt = Range::from(boundary.sub(pages_before * granule)
                    ..boundary.add(pages_after * granule));

                let mut flush = Flush::new();
                unsafe {
                    address_space
                        .map_contiguous::<Size4KiB>(
                            virt,
                            phys,
                            MemoryAttributes::new().with(MemoryAttributes::READ, true),
                            frame_allocator.by_ref(),
                            &physmap,
                            &mut flush,
                        )
                        .unwrap();
                }
                flush.flush(address_space.arch());

                // Every page must be mapped, to the matching physical page, in order.
                for i in 0..total_pages {
                    let page = virt.start.add(i * granule);
                    let mapped = address_space.lookup(page, &physmap);
                    prop_assert!(mapped.is_some(), "page {} is not mapped", page);
                    prop_assert_eq!(mapped.unwrap().0, phys.add(i * granule));
                }
            }
        }
    });
}
