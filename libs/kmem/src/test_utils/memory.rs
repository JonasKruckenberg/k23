use std::alloc::{Allocator, Layout};
use std::collections::BTreeMap;
use std::ops::Range;
use std::ptr::NonNull;
use std::{fmt, mem};

use crate::arch::Arch;
use crate::PhysicalAddress;

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

    pub fn get_region_containing(&self, address: PhysicalAddress) -> Option<(&[u8], usize)> {
        let (_end, (start, region, _)) = self.regions.range(address..).next()?;
        let offset = address.offset_from_unsigned(*start);

        let region = unsafe { region.as_ref() };

        Some((region, offset))
    }

    pub fn get_region_containing_mut(
        &mut self,
        address: PhysicalAddress,
    ) -> Option<(&mut [u8], usize)> {
        let (_end, (start, region, _)) = self.regions.range_mut(address..).next()?;
        let offset = address.get().checked_sub(start.get())?;

        let region = unsafe { region.as_mut() };

        Some((region, offset))
    }

    pub unsafe fn read<T>(&self, address: PhysicalAddress) -> T {
        let size = size_of::<T>();
        if let Some((region, offset)) = self.get_region_containing(address)
            && offset + size <= region.len()
        {
            unsafe { region.as_ptr().add(offset).cast::<T>().read() }
        } else {
            core::panic!("Memory::read: {address} size {size:#x} outside of memory ({self:?})");
        }
    }

    pub unsafe fn write<T>(&mut self, address: PhysicalAddress, value: T) {
        let size = size_of::<T>();
        if let Some((region, offset)) = self.get_region_containing_mut(address)
            && offset + size <= region.len()
        {
            unsafe { region.as_mut_ptr().add(offset).cast::<T>().write(value) };
        } else {
            core::panic!("Memory::write: {address} size {size:#x} outside of memory ({self:?})");
        }
    }

    pub fn write_bytes(&mut self, address: PhysicalAddress, value: u8, count: usize) {
        if let Some((region, offset)) = self.get_region_containing_mut(address)
            && offset + count <= region.len()
        {
            region[offset..offset + count].fill(value);
        } else {
            core::panic!(
                "Memory::write_bytes: {address} size {count:#x} outside of memory ({self:?})"
            );
        }
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
