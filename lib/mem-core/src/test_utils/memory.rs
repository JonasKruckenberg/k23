// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::alloc::Layout;
use std::ptr::NonNull;
use std::range::Range;
use std::{fmt, mem};

use crate::arch::Arch;
use crate::{AddressRangeExt, PhysicalAddress};

pub struct Memory {
    // Regions sorted ascending by end address. A `Vec` + linear scan is deliberate
    // over a `BTreeMap`: there are only a handful of regions, and Miri interprets a
    // flat scan far faster than B-tree node navigation, which dominated test time.
    regions: Vec<(Range<PhysicalAddress>, NonNull<[u8]>, Layout)>,
}

impl Drop for Memory {
    fn drop(&mut self) {
        let regions = mem::take(&mut self.regions);

        for (_range, region, layout) in regions {
            // Safety: we allocated the pointer with the layout at construction time
            unsafe { host_dealloc(region, layout) }
        }
    }
}

impl Memory {
    pub fn new<A: Arch>(region_sizes: impl IntoIterator<Item = Layout>) -> Self {
        let mut regions: Vec<(Range<PhysicalAddress>, NonNull<[u8]>, Layout)> = region_sizes
            .into_iter()
            .map(|layout| {
                let region = host_alloc(layout);

                // Safety: we just allocated the ptr, we know it is valid
                let Range { start, end } = Range::from(unsafe { region.as_ref() }.as_ptr_range());

                let bounds =
                    Range::from(PhysicalAddress::from_ptr(start)..PhysicalAddress::from_ptr(end));

                (bounds, region, layout)
            })
            .collect();

        // Sort by end so `get_region_containing` can take the first region whose
        // end is past the requested start, matching the previous BTreeMap order.
        regions.sort_unstable_by_key(|(bounds, ..)| bounds.end);

        Self { regions }
    }

    pub fn regions(&self) -> impl Iterator<Item = Range<PhysicalAddress>> {
        self.regions.iter().map(|(bounds, _, _)| *bounds)
    }

    fn get_region_containing(
        &self,
        range: Range<PhysicalAddress>,
    ) -> Option<(NonNull<[u8]>, usize)> {
        // First region whose end is strictly past the start of the request,
        // equivalent to the old `range(range.start.add(1)..).next()`.
        let (bounds, region, _) = self
            .regions
            .iter()
            .find(|(bounds, ..)| bounds.end > range.start)?;

        let offset = range.start.get().checked_sub(bounds.start.get())?;

        if offset + range.len() > region.len() {
            return None;
        }

        Some((*region, offset))
    }

    /// # Panics
    ///
    /// Panics if the requested range is not a valid physical memory range.
    #[expect(
        clippy::mut_from_ref,
        reason = "emulated physical memory aliases through raw pointers (like real MMIO); callers are responsible for not creating overlapping mutable views"
    )]
    pub fn region(&self, range: Range<PhysicalAddress>, will_write: bool) -> &mut [u8] {
        let Some((mut region, offset)) = self.get_region_containing(range) else {
            let access_ty = if will_write { "write" } else { "read" };

            panic!(
                "Memory Violation: {access_ty} at {range:?} ({} bytes) outside of memory ({self:?})",
                range.len()
            )
        };

        // Safety: `region` is a live allocation from `host_alloc`, valid for the lifetime of this
        // `Memory`.
        let region = unsafe { region.as_mut() };
        &mut region[offset..offset + range.len()]
    }

    pub unsafe fn read<T>(&self, address: PhysicalAddress) -> T {
        let size = size_of::<T>();
        let region = self.region(Range::from_start_len(address, size), false);

        // Safety: ensured by caller
        unsafe { region.as_ptr().cast::<T>().read() }
    }

    pub unsafe fn write<T>(&self, address: PhysicalAddress, value: T) {
        let size = size_of::<T>();
        let region = self.region(Range::from_start_len(address, size), true);

        // Safety: ensured by caller
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
                    .entries(
                        self.regions
                            .iter()
                            .map(|(bounds, _, _)| bounds.start..bounds.end),
                    )
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
    // Safety: `mmap` with NUL will ask the kernel to choose a memory range => safe.
    // invalid requests yield `MAP_FAILED`, which is asserted against below.
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
        ptr.cast::<u8>(),
        layout.size(),
    ))
    .unwrap()
}

#[cfg(all(unix, not(miri)))]
unsafe fn host_dealloc(region: NonNull<[u8]>, _layout: Layout) {
    // Safety: ensured by caller
    let ret = unsafe { libc::munmap(region.as_ptr().cast::<libc::c_void>(), region.len()) };
    assert_eq!(ret, 0_i32, "munmap failed");
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
