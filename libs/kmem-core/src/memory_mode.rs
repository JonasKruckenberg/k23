// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

// ===== Memory Mode =====
// ===== Prescribing how virtual addresses are translated to physical ones. Describing the shape of the page table =====

use core::alloc::Layout;
use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::ptr;

use arrayvec::ArrayVec;

use crate::arch::Arch;
use crate::{PhysicalAddress, VirtualAddress};

#[repr(C)]
#[derive(Debug)]
pub struct MemoryMode {
    levels: ArrayVec<PageTableLevel, 6>,
    description: &'static str,
    physmap_base: VirtualAddress,
    virtual_address_bits: u8,
}

impl MemoryMode {
    pub const fn description(&self) -> &'static str {
        self.description
    }

    pub const fn levels(&self) -> &[PageTableLevel] {
        self.levels.as_slice()
    }

    pub const fn physmap_base(&self) -> VirtualAddress {
        self.physmap_base
    }

    pub const fn page_size(&self) -> usize {
        if let Some(level) = self.levels.as_slice().last() {
            level.page_size()
        } else {
            panic!()
        }
    }

    pub const fn page_layout(&self) -> Layout {
        if let Ok(layout) = Layout::from_size_align(self.page_size(), self.page_size()) {
            layout
        } else {
            panic!()
        }
    }

    pub const fn virtual_address_bits(&self) -> u8 {
        self.virtual_address_bits
    }
}

#[derive(Debug)]
pub struct PageTableLevel {
    /// The number of entries in this page table level
    entries: usize,
    /// Whether this page table level supports leaf entries.
    supports_leaf: bool,
    /// The number of bits we need to right-shift a `[VirtualAddress`] by to
    /// obtain its PTE index for this level. Used by [`Self::pte_index_of`].
    index_shift: u32,
}

impl PageTableLevel {
    /// Returns the number of page table entries of a table at this level.
    ///
    /// On most architectures all tables - regardless of their level - have the same
    /// number of entries. One notable exception is AArch64 where 16KiB and 64KiB
    /// page size modes have varying numbers of entries per table.
    pub const fn entries(&self) -> usize {
        self.entries
    }

    /// Returns whether this page table level supports leaf entries.
    ///
    /// Leaf entries directly map physical memory, as opposed to pointing
    /// to the next level of the page table hierarchy.
    pub const fn supports_leaf(&self) -> bool {
        self.supports_leaf
    }

    /// The size in bytes of the memory region covered by a page table entry at this level.
    ///
    /// For example, in a 4KiB page system with 512 entries per level:
    /// - Level 0 (leaf): 4KiB (2^12)
    /// - Level 1: 2MiB (2^21)
    /// - Level 2: 1GiB (2^30)
    ///
    /// For an in-depth discussion of page sizes, block sizes, and how the naming conventions used
    /// by different architectures relate to k23's naming, see the [crate-level documentation](crate#page-size-vs-block-size).
    pub const fn page_size(&self) -> usize {
        1 << self.index_shift
    }

    /// Extracts the page table entry (PTE) for a table at this level from the given address.
    pub(crate) fn pte_index_of(&self, address: VirtualAddress) -> usize {
        let idx = (address.get() >> self.index_shift) & (self.entries - 1);
        debug_assert!(idx < self.entries);
        idx
    }

    pub(crate) const fn can_map(
        &self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        len: usize,
    ) -> bool {
        let page_size = self.page_size();
        virt.is_aligned_to(page_size)
            && phys.is_aligned_to(page_size)
            && len >= page_size
            && self.supports_leaf
    }
}

/// Used to indicate that a partially-constructed [`MemoryMode`] is missing its physmap.
pub struct MissingPhysmap;

/// Used to indicate that a partially-constructed [`MemoryMode`] has its azimuthal physmap set.
pub struct HasPhysmap;

/// Used to indicate that a partially-constructed [`Bearing`] is missing its page tables levels.
pub struct MissingLevels;

/// Used to indicate that a partially-constructed [`Bearing`] has its page tables levels set.
pub struct HasLevels;

#[repr(transparent)]
pub struct MemoryModeBuilder<A, Levels, Physmap> {
    under_construction: MemoryMode,
    _marker: PhantomData<A>,
    _has: PhantomData<(Levels, Physmap)>,
}

impl<A: Arch> MemoryModeBuilder<A, MissingLevels, MissingPhysmap> {
    pub const fn new(description: &'static str) -> Self {
        MemoryModeBuilder {
            under_construction: MemoryMode {
                description,
                levels: ArrayVec::new(),
                physmap_base: VirtualAddress::MIN,
                virtual_address_bits: 0,
            },
            _marker: PhantomData,
            _has: PhantomData,
        }
    }
}

impl<A: Arch, Levels> MemoryModeBuilder<A, Levels, MissingPhysmap> {
    pub const fn with_physmap(
        mut self,
        physmap_base: VirtualAddress,
    ) -> MemoryModeBuilder<A, Levels, HasPhysmap> {
        self.under_construction.physmap_base = physmap_base;

        MemoryModeBuilder {
            under_construction: self.into_result(),
            _marker: PhantomData,
            _has: PhantomData,
        }
    }
}

impl<A: Arch, Levels, Physmap> MemoryModeBuilder<A, Levels, Physmap> {
    pub const fn with_level(
        mut self,
        page_size: usize,
        entries: usize,
        supports_leaf: bool,
    ) -> MemoryModeBuilder<A, HasLevels, Physmap> {
        let lvl = PageTableLevel {
            entries,
            supports_leaf,
            index_shift: page_size.ilog2(),
        };

        assert!(self.under_construction.levels.try_push(lvl).is_ok());

        MemoryModeBuilder {
            under_construction: self.into_result(),
            _marker: PhantomData,
            _has: PhantomData,
        }
    }

    const fn into_result(self) -> MemoryMode {
        let me = ManuallyDrop::new(self);
        // Safety: the `repr(C)` on MemoryModeBuilder ensured the MemoryMode is the first field
        // and casting to it is therefore safe. We also know that the location is properly aligned
        // and initialized.
        unsafe { ptr::from_ref(&me).cast::<MemoryMode>().read() }
    }
}

impl<A: Arch> MemoryModeBuilder<A, HasLevels, HasPhysmap> {
    #[expect(clippy::cast_possible_truncation, reason = "not available in const-expressions")]
    pub const fn finish(self) -> MemoryMode {
        let mut result = self.into_result();

        sort_levels(result.levels.as_mut_slice());

        result.virtual_address_bits =
            (result.levels()[0].entries().ilog2() + result.levels()[0].index_shift) as u8;

        result
    }
}

const fn sort_levels(arr: &mut [PageTableLevel]) {
    loop {
        let mut swapped = false;
        let mut i = 1;
        while i < arr.len() {
            if arr[i - 1].page_size() < arr[i].page_size() {
                arr.swap(i - 1, i);
                swapped = true;
            }
            i += 1;
        }
        if !swapped {
            break;
        }
    }
}
