// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::cmp;
use core::range::Range;

use crate::{PhysicalAddress, VirtualAddress};

/// Describes the region of virtual memory that maps all of physical memory. This region is used
/// by the virtual memory subsystem to access memory where only the physical address is known (e.g.
/// zeroing frames of memory in the frame allocator).
///
/// This region must be mapped so it is only accessible by the kernel.
#[derive(Debug, Clone)]
pub struct PhysMap {
    translation_offset: isize,
    range_phys: Range<PhysicalAddress>,
    range_virt: Range<VirtualAddress>,
}

impl PhysMap {
    /// Construct a new `PhysMap` from a chosen base address and the machines physical memory regions.
    /// The iterator over the memory regions must not be empty.
    ///
    /// # Panics
    ///
    /// Panics if the iterator is empty.
    pub fn new(
        physmap_base: VirtualAddress,
        regions: impl IntoIterator<Item = Range<PhysicalAddress>>,
    ) -> Self {
        let mut range_phys = Range::from(PhysicalAddress::MAX..PhysicalAddress::MIN);

        for region in regions {
            range_phys.start = cmp::min(range_phys.start, region.start);
            range_phys.end = cmp::max(range_phys.end, region.end);
        }

        assert!(!range_phys.is_empty(), "regions must not be empty");

        #[expect(
            clippy::cast_possible_wrap,
            reason = "this is expected to wrap when the physmap_start is lower than the lowest physical address (e.g. when it is in upper half of memory)"
        )]
        let translation_offset = physmap_base.get().wrapping_sub(range_phys.start.get()) as isize;

        let range_virt = {
            let start =
                VirtualAddress::new(range_phys.start.wrapping_offset(translation_offset).get());
            let end = VirtualAddress::new(range_phys.end.wrapping_offset(translation_offset).get());

            Range::from(start..end)
        };

        Self {
            translation_offset,
            range_phys,
            range_virt,
        }
    }

    /// Construct a new `PhysMap` that **identity maps** physical memory addresses to virtual addresses.
    ///
    /// The iterator over the memory regions must not be empty.
    ///
    /// # Panics
    ///
    /// Panics if the iterator is empty.
    pub fn new_identity(regions: impl IntoIterator<Item = Range<PhysicalAddress>>) -> Self {
        let mut range_phys = Range::from(PhysicalAddress::MAX..PhysicalAddress::MIN);

        for region in regions {
            range_phys.start = cmp::min(range_phys.start, region.start);
            range_phys.end = cmp::max(range_phys.end, region.end);
        }

        assert!(!range_phys.is_empty(), "regions must not be empty");

        let range_virt = {
            let start = VirtualAddress::new(range_phys.start.get());
            let end = VirtualAddress::new(range_phys.end.get());

            Range::from(start..end)
        };

        Self {
            translation_offset: 0,
            range_phys,
            range_virt,
        }
    }

    /// Translates a `PhysicalAddress` to a `VirtualAddress` through this `PhysMap`.
    ///
    /// # Panics
    ///
    /// Panics if `phys` is _outside_ the physical memory regions this physmap was created with.
    #[inline]
    pub fn phys_to_virt(&self, phys: PhysicalAddress) -> VirtualAddress {
        debug_assert!(
            self.range_phys.contains(&phys),
            "invalid physical address. this is a bug! physmap={self:#x?},phys={phys:?}"
        );

        // Safety: we have checked the address to be within the physical range above
        let virt = unsafe { self.phys_to_virt_internal(phys) };

        debug_assert!(
            self.range_virt.contains(&virt),
            "physical address is not mapped in physical memory mapping. this is a bug! physmap={self:#x?},phys={phys:?},virt={virt}"
        );

        virt
    }

    /// Translates a `Range<PhysicalAddress>` through this `PhysMap`.
    ///
    /// # Panics
    ///
    /// Panics if any address in `phys` range is _outside_ the physical memory regions this physmap was created with.
    #[inline]
    pub fn phys_to_virt_range(&self, phys: Range<PhysicalAddress>) -> Range<VirtualAddress> {
        debug_assert!(
            phys.start >= self.range_phys.start && phys.end <= self.range_phys.end,
            "physical range out of bounds. this is a bug! physmap={self:#x?},phys={phys:?}"
        );

        let virt = {
            // Safety: we checked the range bound to be within the physical range above
            let start = unsafe { self.phys_to_virt_internal(phys.start) };
            // Safety: we checked the range bound to be within the physical range above
            let end = unsafe { self.phys_to_virt_internal(phys.end) };

            Range::from(start..end)
        };

        debug_assert!(
            virt.start >= self.range_virt.start && virt.end <= self.range_virt.end,
            "physical address range not mapped in physical memory mapping. this is a bug! physmap={self:#x?},phys={phys:?},virt={virt:?}"
        );

        virt
    }

    /// Translates a `PhysicalAddress` to a `VirtualAddress` through this `PhysMap` _without_
    /// doing bounds checking of the address.
    ///
    /// # Safety
    ///
    /// 1. The physical address must be contained within one of the physical memory regions this physmap was created with.
    ///    Violating this yields a `VirtualAddress` not backed by any mapping; dereferencing it is undefined behavior.
    #[inline]
    unsafe fn phys_to_virt_internal(&self, phys: PhysicalAddress) -> VirtualAddress {
        VirtualAddress::new(phys.wrapping_offset(self.translation_offset).get())
    }

    /// The virtual address range covered by this physmap.
    pub fn range_virt(&self) -> Range<VirtualAddress> {
        self.range_virt
    }

    /// The physical address range covered by this physmap.
    pub fn range_phys(&self) -> Range<PhysicalAddress> {
        self.range_phys
    }
}
