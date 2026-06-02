// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::alloc::Layout;
use std::collections::BTreeMap;
use std::ptr::NonNull;
use std::range::Range;
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
            unsafe { host_dealloc(region, layout) }
        }
    }
}

impl Memory {
    pub fn new<A: Arch>(region_sizes: impl IntoIterator<Item = Layout>) -> Self {
        let regions = region_sizes
            .into_iter()
            .map(|layout| {
                let region = host_alloc(layout);

                // Safety: we just allocated the ptr, we know it is valid
                let Range { start, end } = Range::from(unsafe { region.as_ref() }.as_ptr_range());

                (
                    PhysicalAddress::from_ptr(end),
                    (PhysicalAddress::from_ptr(start), region, layout),
                )
            })
            .collect();

        Self { regions }
    }

    pub fn regions(&self) -> impl Iterator<Item = Range<PhysicalAddress>> {
        self.regions
            .iter()
            .map(|(end, (start, _, _))| Range::from(*start..*end))
    }

    fn get_region_containing(
        &self,
        range: Range<PhysicalAddress>,
    ) -> Option<(NonNull<[u8]>, usize)> {
        let (_end, (start, region, _)) = self.regions.range(range.start.add(1)..).next()?;

        let offset = range.start.get().checked_sub(start.get())?;

        if offset + range.len() > region.len() {
            return None;
        }

        Some((*region, offset))
    }

    pub fn region(&self, range: Range<PhysicalAddress>, will_write: bool) -> &mut [u8] {
        let Some((mut region, offset)) = self.get_region_containing(range) else {
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

// Use mmap with MAP_NORESERVE on Unix so the kernel doesn't count test memory against
// overcommit limits. Without this, proptests that allocate hundreds of GiB of virtual
// address space fail on Linux systems with limited RAM and no swap.
// Miri can't call `mmap`, so under `cfg(miri)` it uses the `System` branch below.
#[cfg(all(unix, not(miri)))]
fn host_alloc(layout: Layout) -> NonNull<[u8]> {
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            layout.size(),
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    assert_ne!(ptr, libc::MAP_FAILED, "mmap failed for {layout:?}");
    NonNull::new(std::ptr::slice_from_raw_parts_mut(
        ptr as *mut u8,
        layout.size(),
    ))
    .unwrap()
}

#[cfg(all(unix, not(miri)))]
unsafe fn host_dealloc(region: NonNull<[u8]>, _layout: Layout) {
    let ret = unsafe { libc::munmap(region.as_ptr() as *mut libc::c_void, region.len()) };
    assert_eq!(ret, 0, "munmap failed");
}

#[cfg(any(not(unix), miri))]
fn host_alloc(layout: Layout) -> NonNull<[u8]> {
    // Safety: guaranteed by Layout
    let base = NonNull::new(unsafe { std::alloc::alloc(layout) }).unwrap();
    NonNull::slice_from_raw_parts(base, layout.size())
}

#[cfg(any(not(unix), miri))]
unsafe fn host_dealloc(region: NonNull<[u8]>, layout: Layout) {
    // Safety: guaranteed by Layout and caller
    unsafe { std::alloc::dealloc(region.as_ptr().cast::<u8>(), layout) }
}
