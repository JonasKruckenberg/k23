use core::cmp;
use core::ops::Range;

use crate::{PhysicalAddress, VirtualAddress};

/// Describes the region of virtual memory that maps all of physical memory. This region is used
/// by the virtual memory subsystem to access memory where only the physical address is known (e.g.
/// zeroing frames of memory in the frame allocator).
///
/// This region must be mapped so it is only accessible by the kernel.
#[derive(Debug, Clone)]
pub struct PhysMap {
    translation_offset: isize,
    #[cfg(debug_assertions)]
    range: Option<Range<u128>>,
}

impl PhysMap {
    /// Construct a new `PhysMap` from a chosen base address and the machines physical memory regions.
    /// The iterator over the memory regions must not be empty.
    ///
    /// # Panics
    ///
    /// Panics if the iterator is empty.
    pub fn new(
        physmap_start: VirtualAddress,
        regions: impl IntoIterator<Item = Range<PhysicalAddress>>,
    ) -> Self {
        let mut min_addr = PhysicalAddress::MAX;
        let mut max_addr = PhysicalAddress::MIN;

        for region in regions {
            min_addr = cmp::min(min_addr, region.start);
            max_addr = cmp::max(max_addr, region.end);
        }

        assert!(min_addr <= max_addr, "regions must not be empty");

        #[expect(
            clippy::cast_possible_wrap,
            reason = "this is expected to wrap when the physmap_start is lower than the lowest physical address (e.g. when it is in upper half of memory)"
        )]
        let translation_offset = physmap_start.get().wrapping_sub(min_addr.get()) as isize;

        #[cfg(debug_assertions)]
        let range = {
            let start = physmap_start.get() as u128;
            let end = start + max_addr.offset_from_unsigned(min_addr) as u128;

            start..end
        };

        Self {
            translation_offset,
            #[cfg(debug_assertions)]
            range: Some(range),
        }
    }

    pub(crate) const fn new_bootstrap() -> Self {
        Self {
            translation_offset: 0,
            #[cfg(debug_assertions)]
            range: None,
        }
    }

    /// Translates a `PhysicalAddress` to a `VirtualAddress` through this `PhysMap`.
    #[expect(clippy::missing_panics_doc, reason = "internal assert")]
    #[inline]
    pub fn phys_to_virt(&self, phys: PhysicalAddress) -> VirtualAddress {
        let virt = VirtualAddress::new(phys.wrapping_offset(self.translation_offset).get());

        #[cfg(debug_assertions)]
        if let Some(range) = &self.range {
            assert!(
                range.start <= virt.get() as u128 && virt.get() as u128 <= range.end,
                "physical address is not mapped in physical memory mapping. this is a bug! physmap={self:#x?},phys={phys:?},virt={virt}"
            );
        }

        virt
    }

    #[inline]
    pub fn phys_to_virt_range(&self, phys: Range<PhysicalAddress>) -> Range<VirtualAddress> {
        let start = self.phys_to_virt(phys.start);
        let end = self.phys_to_virt(phys.end);

        start..end
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::address_range::AddressRangeExt;
    use crate::test_utils::proptest::{
        aligned_phys, aligned_virt, pick_address_in_regions, regions_phys,
    };
    use crate::{GIB, KIB};

    proptest! {
        #[test]
        fn single_region(base in aligned_virt(any::<VirtualAddress>(), 1*GIB), region_start in aligned_phys(any::<PhysicalAddress>(), 4*KIB), region_size in 0..256*GIB) {
            let map = PhysMap::new(
                base,
                [Range::from_start_len(region_start, region_size)],
            );

            prop_assert_eq!(map.translation_offset, base.get().wrapping_sub(region_start.get()) as isize);
            #[cfg(debug_assertions)]
            prop_assert_eq!(
                map.range,
                Some(base.get() as u128..base.add(region_size).get() as u128)
            )
        }

        #[test]
        fn multi_region(base in aligned_virt(any::<VirtualAddress>(), 1*GIB), regions in regions_phys(1..10, 4*KIB, 256*GIB, 256*GIB)) {
            let regions_start = regions[0].start;

            let map = PhysMap::new(
                base,
                regions
            );

            prop_assert_eq!(map.translation_offset, base.get().wrapping_sub(regions_start.get()) as isize);
        }

        #[test]
        fn phys_to_virt(base in aligned_virt(any::<VirtualAddress>(), 1*GIB), (regions, phys) in pick_address_in_regions(regions_phys(1..10, 4*KIB, 256*GIB, 256*GIB)), ) {
            let regions_start = regions[0].start;

            let map = PhysMap::new(
                base,
                regions
            );

            let virt = map.phys_to_virt(phys);

            prop_assert_eq!(virt.get(), base.get() + (phys.get() - regions_start.get()))
        }
    }

    #[test]
    #[should_panic]
    fn construct_no_regions() {
        let _map = PhysMap::new(VirtualAddress::new(0xffffffc000000000), []);
    }

    #[test]
    fn phys_to_virt_lower_half() {
        let map = PhysMap::new(
            VirtualAddress::new(0x0),
            [PhysicalAddress::new(0x00007f87024d9000)..PhysicalAddress::new(0x00007fc200e17000)],
        );

        println!("{map:?}");

        let virt = map.phys_to_virt(PhysicalAddress::new(0x00007f87024d9000));
        assert_eq!(virt, VirtualAddress::new(0x0));
    }

    #[test]
    fn phys_to_virt_upper_half() {
        let map = PhysMap::new(
            VirtualAddress::new(0xffffffc000000000),
            [PhysicalAddress::new(0x00007f87024d9000)..PhysicalAddress::new(0x00007fc200e17000)],
        );

        println!("{map:?}");

        let virt = map.phys_to_virt(PhysicalAddress::new(0x00007f87024d9000));
        assert_eq!(virt, VirtualAddress::new(0xffffffc000000000));
    }
}
