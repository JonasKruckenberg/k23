use core::cmp;
use core::ops::Range;

use crate::{AddressRangeExt, PhysicalAddress, VirtualAddress};

#[derive(Debug, Clone)]
pub struct PhysicalMemoryMapping {
    translation_offset: usize,
    #[cfg(debug_assertions)]
    range: Option<Range<VirtualAddress>>,
}

impl PhysicalMemoryMapping {
    pub fn new(
        physmap_start: VirtualAddress,
        regions: impl Iterator<Item = Range<PhysicalAddress>>,
    ) -> Self {
        let mut min_addr = PhysicalAddress::MAX;
        let mut max_addr = PhysicalAddress::MIN;

        for region in regions {
            min_addr = cmp::min(min_addr, region.start);
            max_addr = cmp::max(max_addr, region.end);
        }

        assert!(min_addr <= max_addr);

        let translation_offset = physmap_start.get() - min_addr.get();

        #[cfg(debug_assertions)]
        let range = Range::from_start_len(physmap_start, max_addr.offset_from_unsigned(min_addr));

        Self {
            translation_offset,
            #[cfg(debug_assertions)]
            range: Some(range),
        }
    }

    pub(crate) const fn new_bootstrap() -> Self {
        Self {
            translation_offset: 0,
            range: None,
        }
    }

    #[inline]
    pub fn phys_to_virt(&self, phys: PhysicalAddress) -> VirtualAddress {
        let virt = VirtualAddress::new(phys.get() + self.translation_offset);

        #[cfg(debug_assertions)]
        if let Some(range) = &self.range {
            assert!(
                range.start <= virt && virt <= range.end,
                "physical address is not mapped in physical memory mapping. this is a bug! physmap={self:?},phys={phys:?},virt={virt}"
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
    //
    // #[inline]
    // pub fn with_mapped<R>(&self, phys: PhysicalAddress, cb: impl FnOnce(VirtualAddress) -> R) -> R {
    //     let virt = if let Some(physmap) = &self.range {
    //         let virt = physmap.start.add(phys.get());
    //
    //         debug_assert!(physmap.contains(&virt));
    //
    //         virt
    //     } else {
    //         // Safety: during bootstrap no address translation takes place meaning physical addresses *are*
    //         // virtual addresses.
    //         unsafe { mem::transmute::<PhysicalAddress, VirtualAddress>(phys) }
    //     };
    //
    //     cb(virt)
    // }
    //
    // #[inline]
    // pub fn with_mapped_range<R>(
    //     &self,
    //     phys: Range<PhysicalAddress>,
    //     cb: impl FnOnce(Range<VirtualAddress>) -> R,
    // ) -> R {
    //     let virt = if let Some(physmap) = &self.range {
    //         let start = physmap.start.add(phys.start.get());
    //         let end = physmap.start.add(phys.end.get());
    //
    //         debug_assert!(
    //             physmap.contains(&start),
    //             "physical address is not mapped in physical memory mapping. this is a bug! physmap={physmap:?},phys={phys:?},virt={:?}",
    //             start..end
    //         );
    //         debug_assert!(
    //             physmap.contains(&end),
    //             "physical address is not mapped in physical memory mapping. this is a bug! physmap={physmap:?},phys={phys:?},virt={:?}",
    //             start..end
    //         );
    //
    //         start..end
    //     } else {
    //         // Safety: during bootstrap no address translation takes place meaning physical addresses *are*
    //         // virtual addresses.
    //         unsafe {
    //             Range {
    //                 start: mem::transmute::<PhysicalAddress, VirtualAddress>(phys.start),
    //                 end: mem::transmute::<PhysicalAddress, VirtualAddress>(phys.end),
    //             }
    //         }
    //     };
    //
    //     cb(virt)
    // }
}
