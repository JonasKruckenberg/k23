//! `proptest` strategies for virtual memory subsystem tests

use std::ops::Range;

use proptest::prelude::{Just, Strategy};

use crate::{AddressRangeExt, PhysicalAddress, VirtualAddress};

/// Produces `VirtualAddress`s in the given range
pub fn virt(range: Range<usize>) -> impl Strategy<Value = VirtualAddress> {
    range.prop_map(|raw| VirtualAddress::new(raw))
}

/// Produces `VirtualAddress`s aligned to the given `alignment`
pub fn aligned_virt(
    addr: impl Strategy<Value = VirtualAddress>,
    alignment: usize,
) -> impl Strategy<Value = VirtualAddress> {
    addr.prop_map(move |value| value.align_down(alignment))
}

/// Produces `PhysicalAddress`s in the given range
pub fn phys(range: Range<usize>) -> impl Strategy<Value = PhysicalAddress> {
    range.prop_map(|raw| PhysicalAddress::new(raw))
}

/// Produces `PhysicalAddress`s aligned to the given `alignment`
pub fn aligned_phys(
    addr: impl Strategy<Value = PhysicalAddress>,
    alignment: usize,
) -> impl Strategy<Value = PhysicalAddress> {
    addr.prop_map(move |value| value.align_down(alignment))
}

pub fn region_sizes(
    num_regions: Range<usize>,
    alignment: usize,
    max_region_size: usize,
) -> impl Strategy<Value = Vec<usize>> {
    proptest::collection::vec(
        // Size of the region (will be aligned)
        alignment..=max_region_size,
        num_regions,
    )
    .prop_map(move |mut regions| {
        regions.iter_mut().for_each(|size| {
            let align_minus_one = unsafe { alignment.unchecked_sub(1) };

            *size = size.wrapping_add(align_minus_one) & 0usize.wrapping_sub(alignment);

            debug_assert_ne!(*size, 0);
        });
        regions
    })
}

/// Produces a set of *sorted*, *non-overlapping* regions of physical memory aligned to `alignment`.
/// Most useful for initializing an emulated machine.
pub fn regions(
    num_regions: Range<usize>,
    alignment: usize,
    max_region_size: usize,
    max_gap_size: usize,
) -> impl Strategy<Value = Vec<Range<PhysicalAddress>>> {
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
/// as [`regions`].
pub fn pick_address_in_regions(
    regions: impl Strategy<Value = Vec<Range<PhysicalAddress>>>,
) -> impl Strategy<Value = (Vec<Range<PhysicalAddress>>, PhysicalAddress)> {
    regions.prop_flat_map(|regions| {
        let r = regions.clone();
        let address = (0..regions.len()).prop_flat_map(move |chosen_region| {
            let range = r[chosen_region].clone();

            (range.start.get()..range.end.get()).prop_map(|raw| PhysicalAddress::new(raw))
        });

        (Just(regions), address)
    })
}
