// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! `proptest` strategies for virtual memory subsystem tests

use std::alloc::Layout;
use std::ops;
use std::range::Range;

use mem_core::{
    AddressRangeExt, MemoryAttributes, PhysicalAddress, VirtualAddress, WriteOrExecute,
};
use proptest::prelude::{Just, Strategy, any};

/// Produces arbitrary `VirtualAddress`s across the whole `usize` range.
///
/// Replaces the former `proptest_derive::Arbitrary` derive on `VirtualAddress`; keeping
/// the strategy here (rather than an `Arbitrary` impl in `mem-core`) keeps `mem-core`
/// free of any `proptest` dependency.
pub fn any_virt() -> impl Strategy<Value = VirtualAddress> {
    any::<usize>().prop_map(VirtualAddress::new)
}

/// Produces arbitrary `PhysicalAddress`s across the whole `usize` range.
pub fn any_phys() -> impl Strategy<Value = PhysicalAddress> {
    any::<usize>().prop_map(PhysicalAddress::new)
}

/// Produces arbitrary *valid* `MemoryAttributes`.
///
/// Generates only valid bit patterns: an arbitrary `u8` would allow the
/// `WRITE_OR_EXECUTE` pattern `0b11`, which has no `WriteOrExecute` variant and panics
/// in `get`. Replaces the former `Arbitrary for MemoryAttributes` impl in `mem-core`.
pub fn attrs() -> impl Strategy<Value = MemoryAttributes> {
    (any::<bool>(), 0u8..3).prop_map(|(read, write_or_execute)| {
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
}

/// Produces `VirtualAddress`s in the given range
#[expect(
    clippy::disallowed_types,
    reason = "proptest's Strategy is implemented for core::ops::Range, not core::range::Range"
)]
pub fn virt(range: ops::Range<usize>) -> impl Strategy<Value = VirtualAddress> {
    range.prop_map(VirtualAddress::new)
}

/// Produces `VirtualAddress`s aligned to the given `alignment`
pub fn aligned_virt(
    addr: impl Strategy<Value = VirtualAddress>,
    alignment: usize,
) -> impl Strategy<Value = VirtualAddress> {
    addr.prop_map(move |value| value.align_down(alignment))
}

/// Produces `PhysicalAddress`s in the given range
#[expect(
    clippy::disallowed_types,
    reason = "proptest's Strategy is implemented for core::ops::Range, not core::range::Range"
)]
pub fn phys(range: ops::Range<usize>) -> impl Strategy<Value = PhysicalAddress> {
    range.prop_map(PhysicalAddress::new)
}

/// Produces `PhysicalAddress`s aligned to the given `alignment`
pub fn aligned_phys(
    addr: impl Strategy<Value = PhysicalAddress>,
    alignment: usize,
) -> impl Strategy<Value = PhysicalAddress> {
    addr.prop_map(move |value| value.align_down(alignment))
}

/// Produces a set of [`Layout`]s for regions of physical memory aligned to `alignment`.
/// Most useful for initializing an emulated machine.
///
/// # Panics
///
/// Panics if `alignment` is not a power of two.
#[expect(
    clippy::disallowed_types,
    reason = "proptest's vec() size argument requires Into<SizeRange>, implemented for core::ops::Range"
)]
pub fn region_layouts(
    num_regions: ops::Range<usize>,
    alignment: usize,
    max_region_size: usize,
) -> impl Strategy<Value = Vec<Layout>> {
    assert!(alignment.is_power_of_two());

    proptest::collection::vec(
        // Size of the region (will be aligned)
        alignment..=max_region_size,
        num_regions,
    )
    .prop_map(move |regions| {
        regions
            .into_iter()
            .map(|size| {
                // Safety: `alignment` is a power of two (asserted above), hence non-zero, so `- 1` cannot underflow
                let align_minus_one = unsafe { alignment.unchecked_sub(1) };

                let size = size.wrapping_add(align_minus_one) & 0usize.wrapping_sub(alignment);

                debug_assert_ne!(size, 0);

                Layout::from_size_align(size, alignment).unwrap()
            })
            .collect()
    })
}

/// Produces a set of *sorted*, *non-overlapping* regions of physical memory aligned to `alignment`.
/// Most useful for initializing an emulated machine.
///
/// # Panics
///
/// Panics if `alignment` is not a power of two.
#[expect(
    clippy::disallowed_types,
    reason = "proptest's vec() size argument requires Into<SizeRange>, implemented for core::ops::Range"
)]
pub fn regions_phys(
    num_regions: ops::Range<usize>,
    alignment: usize,
    max_region_size: usize,
    max_gap_size: usize,
) -> impl Strategy<Value = Vec<Range<PhysicalAddress>>> {
    assert!(alignment.is_power_of_two());

    proptest::collection::vec(
        (
            // Size of the region (will be aligned)
            alignment..=max_region_size,
            // Gap after this region (will be aligned)
            alignment..=max_gap_size,
        ),
        num_regions,
    )
    .prop_flat_map(move |size_gap_pairs| {
        // Calculate the maximum starting address that won't cause overflow
        let max_start = {
            let total_space_needed: usize =
                size_gap_pairs.iter().map(|(size, gap)| size + gap).sum();

            // Ensure we have headroom for alignment adjustments
            usize::MAX
                .saturating_sub(total_space_needed)
                .saturating_sub(alignment)
        };

        (0..=max_start).prop_map(move |start_raw| {
            let mut regions = Vec::with_capacity(size_gap_pairs.len());
            let mut current = PhysicalAddress::new(start_raw).align_down(alignment);

            for (size, gap) in &size_gap_pairs {
                let range: Range<PhysicalAddress> =
                    Range::from_start_len(current, *size).align_in(alignment);
                assert!(!range.is_empty());

                regions.push(range);

                current = current.add(size + gap).align_up(alignment);
            }

            regions
        })
    })
}

/// Picks an arbitrary `PhysicalAddress` from a strategy that produces physical memory regions such
/// as [`regions_phys`].
pub fn pick_address_in_regions(
    regions: impl Strategy<Value = Vec<Range<PhysicalAddress>>>,
) -> impl Strategy<Value = (Vec<Range<PhysicalAddress>>, PhysicalAddress)> {
    regions.prop_flat_map(|regions| {
        let r = regions.clone();
        let address = (0..regions.len()).prop_flat_map(move |chosen_region| {
            let range = r[chosen_region];

            (range.start.get()..range.end.get()).prop_map(PhysicalAddress::new)
        });

        (Just(regions), address)
    })
}

/// Produces a set of *sorted*, *non-overlapping* regions of virtual memory aligned to `alignment`.
///
/// # Panics
///
/// Panics if `alignment` is not a power of two.
#[expect(
    clippy::disallowed_types,
    reason = "proptest's vec() size argument requires Into<SizeRange>, implemented for core::ops::Range"
)]
pub fn regions_virt(
    num_regions: ops::Range<usize>,
    alignment: usize,
    max_region_size: usize,
    max_gap_size: usize,
) -> impl Strategy<Value = Vec<Range<VirtualAddress>>> {
    assert!(alignment.is_power_of_two());

    proptest::collection::vec(
        (
            // Size of the region (will be aligned)
            alignment..=max_region_size,
            // Gap after this region (will be aligned)
            alignment..=max_gap_size,
        ),
        num_regions,
    )
    .prop_flat_map(move |size_gap_pairs| {
        // Calculate the maximum starting address that won't cause overflow
        let max_start = {
            let total_space_needed: usize =
                size_gap_pairs.iter().map(|(size, gap)| size + gap).sum();

            // Ensure we have headroom for alignment adjustments
            usize::MAX
                .saturating_sub(total_space_needed)
                .saturating_sub(alignment)
        };

        (0..=max_start).prop_map(move |start_raw| {
            let mut regions = Vec::with_capacity(size_gap_pairs.len());
            let mut current = VirtualAddress::new(start_raw).align_down(alignment);

            for (size, gap) in &size_gap_pairs {
                let range: Range<VirtualAddress> =
                    Range::from_start_len(current, *size).align_in(alignment);
                assert!(!range.is_empty());

                regions.push(range);

                current = current.add(size + gap).align_up(alignment);
            }

            regions
        })
    })
}
