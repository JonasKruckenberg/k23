// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::range::Range;

use human_bytes::{GIB, KIB};
use mem_core::{PhysMap, PhysicalAddress, Size4KiB, VirtualAddress};
use mem_testkit::proptest::{aligned_virt, any_virt, pick_address_in_regions, regions_phys};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(if cfg!(miri) { 25 } else { 250 }))]

    #[test]
    fn phys_to_virt(base in aligned_virt(any_virt(), 1*GIB), (regions, phys) in pick_address_in_regions(regions_phys(1..10, 4*KIB, 256*GIB, 256*GIB)), ) {
        let regions_start = regions[0].start;

        let map = PhysMap::new::<Size4KiB>(
            base,
            regions
        );

        let virt = map.phys_to_virt(phys);

        prop_assert_eq!(virt.get(), base.get() + (phys.get() - regions_start.get()))
    }
}

// NOTE(mem-core split): `single_region` and `multi_region` assert on the private
// `PhysMap::translation_offset` field, which an out-of-crate integration test cannot
// reach. Left commented for review — expose a `translation_offset` accessor (or re-derive
// the assertion through the public `phys_to_virt`/`range_virt`) to restore them. Strategies
// now come from `mem_testkit::proptest` (`aligned_phys`, `aligned_virt`, `regions_phys`).
/*
proptest! {
    #![proptest_config(ProptestConfig::with_cases(if cfg!(miri) { 25 } else { 250 }))]

    #[test]
    fn single_region(base in aligned_virt(any_virt(), 1*GIB), region_start in aligned_phys(any_phys(), 4*KIB), region_size in 0..256*GIB) {
        let map = PhysMap::new(
            base,
            [Range::from_start_len(region_start, region_size)],
        );

        prop_assert_eq!(map.translation_offset, base.get().wrapping_sub(region_start.get()) as isize);
        #[cfg(debug_assertions)]
        prop_assert_eq!(
            map.range_virt(),
            Range::from_start_len(base, region_size)
        )
    }

    #[test]
    fn multi_region(base in aligned_virt(any_virt(), 1*GIB), regions in regions_phys(1..10, 4*KIB, 256*GIB, 256*GIB)) {
        let regions_start = regions[0].start;

        let map = PhysMap::new(
            base,
            regions
        );

        prop_assert_eq!(map.translation_offset, base.get().wrapping_sub(regions_start.get()) as isize);
    }
}
*/

#[test]
#[should_panic]
fn construct_no_regions() {
    let _map = PhysMap::new::<Size4KiB>(VirtualAddress::new(0xffffffc000000000), []);
}

#[test]
fn phys_to_virt_lower_half() {
    let map = PhysMap::new::<Size4KiB>(
        VirtualAddress::new(0x0),
        [Range::from(
            PhysicalAddress::new(0x00007f87024d9000)..PhysicalAddress::new(0x00007fc200e17000),
        )],
    );

    println!("{map:?}");

    let virt = map.phys_to_virt(PhysicalAddress::new(0x00007f87024d9000));
    assert_eq!(virt, VirtualAddress::new(0x0));
}

#[test]
fn phys_to_virt_upper_half() {
    let map = PhysMap::new::<Size4KiB>(
        VirtualAddress::new(0xffffffc000000000),
        [Range::from(
            PhysicalAddress::new(0x00007f87024d9000)..PhysicalAddress::new(0x00007fc200e17000),
        )],
    );

    println!("{map:?}");

    let virt = map.phys_to_virt(PhysicalAddress::new(0x00007f87024d9000));
    assert_eq!(virt, VirtualAddress::new(0xffffffc000000000));
}
