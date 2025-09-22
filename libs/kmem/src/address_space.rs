// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod region;

use core::alloc::Layout;
use core::ptr::NonNull;

use wavltree::WAVLTree;

use crate::AccessRules;
use crate::address_space::region::AddressSpaceRegion;

pub struct AddressSpace {
    regions: WAVLTree<AddressSpaceRegion>,
}

// ===== impl AddressSpace =====

impl Default for AddressSpace {
    fn default() -> Self {
        Self::new()
    }
}

impl AddressSpace {
    pub const fn new() -> Self {
        todo!()
    }

    /// Attempts to reserve a region of virtual memory.
    ///
    /// On success, returns a [`NonNull<[u8]>`][NonNull] meeting the size and alignment guarantees
    /// of `layout`. Access to this region must obey the provided `rules` or cause a hardware fault.
    ///
    /// The returned region may have a larger size than specified by `layout.size()`, and may or may
    /// not have its contents initialized.
    ///
    /// The returned region of virtual memory remains mapped as long as it is [*currently mapped*]
    /// and the address space type itself has not been dropped.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the layout does not meet the address space's size or alignment
    /// constraints, virtual memory is exhausted, or mapping otherwise fails.
    #[expect(unused, reason = "used by later change")]
    pub fn map<R: lock_api::RawRwLock>(
        &mut self,
        layout: Layout,
        access_rules: AccessRules,
    ) -> crate::Result<NonNull<[u8]>> {
        // self.regions.insert(AddressSpaceRegion::new());

        todo!()
    }

    /// Attempts to extend the virtual memory reservation.
    ///
    /// Returns a new [`NonNull<[u8]>`][NonNull] containing a pointer and the actual size of the
    /// mapped region. The pointer is suitable for holding data described by `new_layout`. To accomplish
    /// this, the address space may extend the mapping referenced by `ptr` to fit the new layout.
    ///
    /// TODO describe how extending a file-backed, of DMA-backed mapping works
    ///
    /// The [`AccessRules`] of the new virtual memory region are *the same* at the old ones.
    ///
    /// If this returns `Ok`, then ownership of the memory region referenced by `ptr` has been
    /// transferred to this address space. Any access to the old `ptr` is [*Undefined Behavior*],
    /// even if the mapping was grown in-place. The newly returned pointer is the only valid pointer
    /// for accessing this region now.
    ///
    /// If this method returns `Err`, then ownership of the memory region has not been transferred to
    /// this address space, and the contents of the region are unaltered.
    ///
    /// [*Undefined Behavior*]
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space.
    /// * `old_layout` must [*fit*] that region (The `new_layout` argument need not fit it.).
    /// * `new_layout.size()` must be greater than or equal to `old_layout.size()`.
    ///
    /// Note that `new_layout.align()` need not be the same as `old_layout.align()`.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the layout does not meet the address space's size or alignment
    /// constraints, virtual memory is exhausted, or growing otherwise fails.
    #[expect(unused, reason = "used by later change")]
    pub unsafe fn grow(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        todo!()
    }

    /// Behaves like [`grow`][AddressSpace::grow], only grows the region if it can be grown in-place.
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space.
    /// * `old_layout` must [*fit*] that region (The `new_layout` argument need not fit it.).
    /// * `new_layout.size()` must be greater than or equal to `old_layout.size()`.
    ///
    /// Note that `new_layout.align()` need not be the same as `old_layout.align()`.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the layout does not meet the address space's size or alignment
    /// constraints, virtual memory is exhausted, or growing otherwise fails.
    #[expect(unused, reason = "used by later change")]
    pub unsafe fn grow_in_place(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        todo!()
    }

    /// Attempts to shrink the virtual memory reservation.
    ///
    /// Returns a new [`NonNull<[u8]>`][NonNull] containing a pointer and the actual size of the
    /// mapped region. The pointer is suitable for holding data described by `new_layout`. To accomplish
    /// this, the address space may shrink the mapping referenced by `ptr` to fit the new layout.
    ///
    /// TODO describe how shrinking a file-backed, of DMA-backed mapping works
    ///
    /// The [`AccessRules`] of the new virtual memory region are *the same* at the old ones.
    ///
    /// If this returns `Ok`, then ownership of the memory region referenced by `ptr` has been
    /// transferred to this address space. Any access to the old `ptr` is [*Undefined Behavior*],
    /// even if the mapping was shrunk in-place. The newly returned pointer is the only valid pointer
    /// for accessing this region now.
    ///
    /// If this method returns `Err`, then ownership of the memory region has not been transferred to
    /// this address space, and the contents of the region are unaltered.
    ///
    /// [*Undefined Behavior*]
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space.
    /// * `old_layout` must [*fit*] that region (The `new_layout` argument need not fit it.).
    /// * `new_layout.size()` must be smaller than or equal to `old_layout.size()`.
    ///
    /// Note that `new_layout.align()` need not be the same as `old_layout.align()`.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the layout does not meet the address space's size or alignment
    /// constraints, virtual memory is exhausted, or shrinking otherwise fails.
    #[expect(unused, reason = "used by later change")]
    pub unsafe fn shrink(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        todo!()
    }

    /// Behaves like [`shrink`][AddressSpace::shrink], but *guarantees* that the region will be
    /// shrunk in-place. Both `old_layout` and `new_layout` need to be at least page aligned.
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space.
    /// * `old_layout` must [*fit*] that region (The `new_layout` argument need not fit it.).
    /// * `new_layout.size()` must be smaller than or equal to `old_layout.size()`.
    ///
    /// Note that `new_layout.align()` need not be the same as `old_layout.align()`.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the layout does not meet the address space's size or alignment
    /// constraints, virtual memory is exhausted, or growing otherwise fails.
    #[expect(unused, reason = "used by later change")]
    pub unsafe fn shrink_in_place(
        &mut self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        todo!()
    }

    /// Unmaps the virtual memory region referenced by `ptr`.
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space, and
    /// * `layout` must [*fit*] that region of memory.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    #[expect(unused, reason = "used by later change")]
    pub unsafe fn unmap(&mut self, ptr: NonNull<u8>, layout: Layout) {
        todo!()
    }

    /// Updates the access rules for the virtual memory region referenced by `ptr`.
    ///
    /// After this returns, access to this region must obey the new `rules` or cause a hardware fault.
    // If this returns `Ok`, access to this region must obey the new `rules` or cause a hardware fault.
    // If this method returns `Err`, the access rules of the memory region are unaltered.
    ///
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space, and
    /// * `layout` must [*fit*] that region of memory.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    #[expect(unused, reason = "used by later change")]
    pub unsafe fn update_access_rules(
        &mut self,
        ptr: NonNull<u8>,
        layout: Layout,
        access_rules: AccessRules,
    ) {
        todo!()
    }

    /// Attempts to fill the virtual memory region referenced by `ptr` with zeroes.
    ///
    /// Returns a new [`NonNull<[u8]>`][NonNull] containing a pointer and the actual size of the
    /// mapped region. The pointer is suitable for holding data described by `new_layout` and is
    /// *guaranteed* to be zero-initialized. To accomplish this, the address space may remap the
    /// virtual memory region.
    ///
    /// TODO describe how clearing a file-backed, of DMA-backed mapping works
    ///
    /// The [`AccessRules`] of the new virtual memory region are *the same* at the old ones.
    ///
    /// If this returns `Ok`, then ownership of the memory region referenced by `ptr` has been
    /// transferred to this address space. Any access to the old `ptr` is [*Undefined Behavior*],
    /// even if the mapping was cleared in-place. The newly returned pointer is the only valid pointer
    /// for accessing this region now.
    ///
    /// If this method returns `Err`, then ownership of the memory region has not been transferred to
    /// this address space, and the contents of the region are unaltered.
    ///
    /// [*Undefined Behavior*]
    ///
    /// # Safety
    ///
    /// * `ptr` must denote a region of memory [*currently mapped*] in this address space, and
    /// * `layout` must [*fit*] that region of memory.
    ///
    /// [*currently mapped*]: #currently-mapped-memory
    /// [*fit*]: #memory-fitting
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the layout does not meet the address space's size or alignment
    /// constraints, clearing a virtual memory region is not supported by the backing storage, or
    /// clearing otherwise fails.
    #[expect(unused, reason = "used by later change")]
    pub unsafe fn clear(
        &mut self,
        ptr: NonNull<u8>,
        layout: Layout,
    ) -> crate::Result<NonNull<[u8]>> {
        todo!()
    }

    pub fn assert_valid(&self, ctx: &str) {
        self.regions.assert_valid(ctx);
    }
}
