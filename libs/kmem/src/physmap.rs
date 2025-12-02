use core::mem;
use core::ops::Range;

use crate::{PhysicalAddress, VirtualAddress};

#[derive(Debug, Clone)]
pub struct PhysicalMemoryMapping {
    range: Option<Range<VirtualAddress>>,
}

impl PhysicalMemoryMapping {
    pub const fn new(range: Range<VirtualAddress>) -> Self {
        Self { range: Some(range) }
    }

    pub(crate) const fn new_bootstrap() -> Self {
        Self { range: None }
    }

    #[inline]
    pub fn with_mapped<R>(&self, phys: PhysicalAddress, cb: impl FnOnce(VirtualAddress) -> R) -> R {
        let virt = if let Some(physmap) = &self.range {
            let virt = physmap.start.add(phys.get());

            debug_assert!(physmap.contains(&virt));

            virt
        } else {
            // Safety: during bootstrap no address translation takes place meaning physical addresses *are*
            // virtual addresses.
            unsafe { mem::transmute::<PhysicalAddress, VirtualAddress>(phys) }
        };

        cb(virt)
    }

    #[inline]
    pub fn with_mapped_range<R>(
        &self,
        phys: Range<PhysicalAddress>,
        cb: impl FnOnce(Range<VirtualAddress>) -> R,
    ) -> R {
        let virt = if let Some(physmap) = &self.range {
            let start = physmap.start.add(phys.start.get());
            let end = physmap.start.add(phys.end.get());

            debug_assert!(physmap.contains(&start), "physical address is not mapped in physical memory mapping. this is a bug! physmap={physmap:?},phys={phys:?},virt={:?}", start..end);
            debug_assert!(physmap.contains(&end), "physical address is not mapped in physical memory mapping. this is a bug! physmap={physmap:?},phys={phys:?},virt={:?}", start..end);

            start..end
        } else {
            // Safety: during bootstrap no address translation takes place meaning physical addresses *are*
            // virtual addresses.
            unsafe {
                Range {
                    start: mem::transmute::<PhysicalAddress, VirtualAddress>(phys.start),
                    end: mem::transmute::<PhysicalAddress, VirtualAddress>(phys.end),
                }
            }
        };

        cb(virt)
    }
}
