use std::alloc::{Allocator, Layout};
use std::collections::BTreeMap;
use std::ops::Range;
use std::ptr::NonNull;
use std::{fmt, mem};

use crate::arch::Arch;
use crate::{AddressRangeExt, PhysicalAddress};

pub struct Memory {
    regions: BTreeMap<PhysicalAddress, (PhysicalAddress, NonNull<[u8]>, Layout)>,
}

impl Drop for Memory {
    fn drop(&mut self) {
        let regions = mem::take(&mut self.regions);

        for (_end, (_start, region, layout)) in regions {
            unsafe { std::alloc::System.deallocate(region.cast(), layout) }
        }
    }
}

impl Memory {
    pub fn new<A: Arch>(region_sizes: impl IntoIterator<Item = usize>) -> Self {
        let regions = region_sizes
            .into_iter()
            .map(|size| {
                let layout = Layout::from_size_align(size, A::GRANULE_SIZE).unwrap();

                let region = std::alloc::System.allocate(layout).unwrap();

                // Safety: we just allocated the ptr, we know it is valid
                let Range { start, end } = unsafe { region.as_ref() }.as_ptr_range();

                (
                    PhysicalAddress::from_ptr(end),
                    (PhysicalAddress::from_ptr(start), region, layout),
                )
            })
            .collect();

        Self { regions }
    }

    pub fn regions(&self) -> impl Iterator<Item = Range<PhysicalAddress>> {
        self.regions.iter().map(|(end, (start, _, _))| *start..*end)
    }

    fn get_region_containing(&self, address: PhysicalAddress) -> Option<(NonNull<[u8]>, usize)> {
        let (_end, (start, region, _)) = self.regions.range(address..).next()?;

        let offset = address.get().checked_sub(start.get())?;

        Some((*region, offset))
    }

    pub fn region(&self, range: Range<PhysicalAddress>, will_write: bool) -> &mut [u8] {
        let Some((mut region, offset)) = self.get_region_containing(range.start) else {
            let access_ty = if will_write { "write" } else { "read" };

            panic!(
                "Memory Violation: {access_ty} at {range:?} ({} bytes) outside of memory ({self:?})",
                range.len()
            )
        };

        let region = unsafe { region.as_mut() };
        &mut region[offset..offset + range.len()]
    }

    pub unsafe fn read<T>(&self, address: PhysicalAddress) -> T {
        let size = size_of::<T>();
        let region = self.region(Range::from_start_len(address, size), false);

        unsafe { region.as_ptr().cast::<T>().read() }
    }

    pub unsafe fn write<T>(&self, address: PhysicalAddress, value: T) {
        let size = size_of::<T>();
        let region = self.region(Range::from_start_len(address, size), true);

        unsafe { region.as_mut_ptr().cast::<T>().write(value) }
    }

    pub fn read_bytes(&self, address: PhysicalAddress, count: usize) -> &[u8] {
        self.region(Range::from_start_len(address, count), false)
    }

    pub fn write_bytes(&self, address: PhysicalAddress, value: u8, count: usize) {
        let region = self.region(Range::from_start_len(address, count), true);

        region.fill(value);
    }
}

impl fmt::Debug for Memory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Memory")
            .field_with("regions", |f| {
                f.debug_list()
                    .entries(self.regions.iter().map(|(end, (start, _, _))| *start..*end))
                    .finish()
            })
            .finish()
    }
}
