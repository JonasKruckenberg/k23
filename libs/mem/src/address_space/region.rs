// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use core::alloc::Layout;
use core::mem::offset_of;
use core::ops::{Range, RangeBounds};
use core::pin::Pin;
use core::ptr::NonNull;
use core::{cmp, mem, slice};

use pin_project::pin_project;

use crate::address_space::batch::Batch;
use crate::addresses::AddressRangeExt;
use crate::{AccessRules, VirtualAddress};

#[pin_project]
#[derive(Debug)]
pub struct AddressSpaceRegion {
    access_rules: AccessRules,
    layout: Layout,
    range: Range<VirtualAddress>,
    /// The address range covered by this region and its WAVL tree subtree, used when allocating new regions
    subtree_range: Range<VirtualAddress>,
    /// The largest gap in this subtree, used when allocating new regions
    max_gap: usize,
    /// Links to other regions in the WAVL tree
    links: wavltree2::Links<AddressSpaceRegion>,
}

impl AddressSpaceRegion {
    pub const fn new(spot: VirtualAddress, layout: Layout, access_rules: AccessRules) -> Self {
        Self {
            range: spot..spot.checked_add(layout.size()).unwrap(),
            access_rules,
            layout,

            max_gap: 0,
            subtree_range: spot..spot.checked_add(layout.size()).unwrap(),
            links: wavltree2::Links::new(),
        }
    }

    pub const fn range(&self) -> &Range<VirtualAddress> {
        &self.range
    }

    pub const fn subtree_range(&self) -> &Range<VirtualAddress> {
        &self.subtree_range
    }

    pub const fn access_rules(&self) -> AccessRules {
        self.access_rules
    }

    pub fn as_slice(&self) -> &[u8] {
        let ptr = self.range.start.as_ptr();
        let len = self.range.size();

        unsafe { slice::from_raw_parts(ptr, len) }
    }

    pub fn as_slice_mut(&mut self) -> &mut [u8] {
        let ptr = self.range.start.as_mut_ptr();
        let len = self.range.size();

        unsafe { slice::from_raw_parts_mut(ptr, len) }
    }

    pub fn as_non_null(&self) -> NonNull<[u8]> {
        let ptr = self.range.start.as_non_null().unwrap();
        NonNull::slice_from_raw_parts(ptr, self.range.size())
    }

    pub const fn layout_fits_region(&self, layout: Layout) -> bool {
        self.range.start.is_aligned_to(layout.align())
            && layout.size() >= self.layout.size()
            && layout.size() <= self.range.end.get() - self.range.start.get()
    }

    /// grow region to `new_len`, attempting to grow the VMO accordingly
    /// `new_layout.size()` mut be greater than or equal to `self.layout.size()`
    pub fn grow_in_place(&mut self, new_layout: Layout, batch: &mut Batch) -> crate::Result<()> {
        // TODO
        //  - attempt to resize VMO
        //  - update self range

        todo!()
    }

    /// shrink region to the first `len` bytes, dropping the rest frames.
    /// `new_layout.size()` mut be smaller than or equal to `self.layout.size()`
    pub fn shrink(&mut self, new_layout: Layout, batch: &mut Batch) -> crate::Result<()> {
        // TODO
        //  - drop rest pages in VMO if possible (add unmaps to batch)
        //  - update self range

        todo!()
    }

    /// move the entire region to the new base address, remapping any already mapped frames
    pub fn move_to(&mut self, base: VirtualAddress, batch: &mut Batch) -> crate::Result<()> {
        // TODO
        //  - attempt to resize VMO
        //  - update self range
        //  - for every frame in VMO
        //      - attempt to map at new offset (add maps to batch)

        todo!()
    }

    pub fn commit<R>(&mut self, range: R, will_write: bool, batch: &mut Batch) -> crate::Result<()>
    where
        R: RangeBounds<VirtualAddress>,
    {
        // TODO
        //  - for every *uncommited* frame in range
        //      - request frame from VMO (add map to batch)

        todo!()
    }

    pub fn decommit<R>(&mut self, range: R, batch: &mut Batch) -> crate::Result<()>
    where
        R: RangeBounds<VirtualAddress>,
    {
        // TODO
        //  - for every *committed* frame in range
        //      - drop pages in VMO if possible (add unmaps to batch)

        todo!()
    }

    /// updates the access rules fo this region
    pub fn update_access_rules(
        &mut self,
        access_rules: AccessRules,
        batch: &mut Batch,
    ) -> crate::Result<()> {
        // TODO
        //  - for every frame in VMO
        //      - update access rules (add protects to batch)
        //  - update self access rules

        todo!()
    }

    pub fn clear(&mut self, batch: &mut Batch) -> crate::Result<()> {
        // TODO
        //  - replace VMO with "zeroed" VMO
        //  - drop pages in VMO if possible (add unmaps to batch)

        todo!()
    }

    pub fn assert_valid(&self, msg: &str) {
        assert!(!self.range.is_empty(), "{msg}region range cannot be empty");
        assert!(
            self.subtree_range.start <= self.range.start
                && self.range.end <= self.subtree_range.end,
            "{msg}region range cannot be bigger than its subtree range; region={self:?}"
        );
        assert!(
            self.max_gap < self.subtree_range.size(),
            "{msg}region's subtree max_gap cannot be bigger than its subtree range; region={self:?}"
        );
        assert!(
            self.range.start.is_aligned_to(self.layout.align()),
            "{msg}region range is not aligned to its layout; region={self:?}"
        );
        assert!(
            self.range.size() >= self.layout.size(),
            "{msg}region range is smaller than its layout; region={self:?}"
        );

        self.links.assert_valid();
    }

    /// Returns `true` if this nodes subtree contains a gap suitable for the given `layout`, used
    /// during gap-searching.
    pub fn suitable_gap_in_subtree(&self, layout: Layout) -> bool {
        // we need the layout to be padded to alignment
        debug_assert!(layout.size().is_multiple_of(layout.align()));

        self.max_gap >= layout.size()
    }

    /// Returns the left child node in the search tree of regions, used during gap-searching.
    pub fn left_child(&self) -> Option<&Self> {
        Some(unsafe { self.links.left()?.as_ref() })
    }

    /// Returns the right child node in the search tree of regions, used during gap-searching.
    pub fn right_child(&self) -> Option<&Self> {
        Some(unsafe { self.links.right()?.as_ref() })
    }

    /// Returns the parent node in the search tree of regions, used during gap-searching.
    pub fn parent(&self) -> Option<&Self> {
        Some(unsafe { self.links.parent()?.as_ref() })
    }

    /// Update the gap search metadata of this region. This method is called in the [`wavltree::Linked`]
    /// implementation below after each tree mutation that impacted this node or its subtree in some way
    /// (insertion, rotation, deletion).
    ///
    /// Returns `true` if this nodes metadata changed.
    #[expect(clippy::undocumented_unsafe_blocks, reason = "intrusive tree access")]
    fn update_gap_metadata(
        mut node: NonNull<Self>,
        left: Option<NonNull<Self>>,
        right: Option<NonNull<Self>>,
    ) -> bool {
        fn gap(left_last_byte: VirtualAddress, right_first_byte: VirtualAddress) -> usize {
            right_first_byte
                .checked_sub_addr(left_last_byte)
                .unwrap_or_default() // TODO use saturating_sub_addr
        }

        let node = unsafe { node.as_mut() };
        let mut left_max_gap = 0;
        let mut right_max_gap = 0;

        // recalculate the subtree_range start
        let old_subtree_range_start = if let Some(left) = left {
            let left = unsafe { left.as_ref() };
            let left_gap = gap(left.subtree_range.end, node.range.start);
            left_max_gap = cmp::max(left_gap, left.max_gap);
            mem::replace(&mut node.subtree_range.start, left.subtree_range.start)
        } else {
            mem::replace(&mut node.subtree_range.start, node.range.start)
        };

        // recalculate the subtree range end
        let old_subtree_range_end = if let Some(right) = right {
            let right = unsafe { right.as_ref() };
            let right_gap = gap(node.range.end, right.subtree_range.start);
            right_max_gap = cmp::max(right_gap, right.max_gap);
            mem::replace(&mut node.subtree_range.end, right.subtree_range.end)
        } else {
            mem::replace(&mut node.subtree_range.end, node.range.end)
        };

        // recalculate the map_gap
        let old_max_gap = mem::replace(&mut node.max_gap, cmp::max(left_max_gap, right_max_gap));

        old_max_gap != node.max_gap
            || old_subtree_range_start != node.subtree_range.start
            || old_subtree_range_end != node.subtree_range.end
    }

    // Propagate metadata updates to this regions parent in the search tree. If we had to update
    // our metadata the parent must update its metadata too.
    #[expect(clippy::undocumented_unsafe_blocks, reason = "intrusive tree access")]
    fn propagate_update_to_parent(mut maybe_node: Option<NonNull<Self>>) {
        while let Some(node) = maybe_node {
            let links = unsafe { &node.as_ref().links };
            let changed = Self::update_gap_metadata(node, links.left(), links.right());

            // if the metadata didn't actually change, we don't need to recalculate parents
            if !changed {
                return;
            }

            maybe_node = links.parent();
        }
    }
}

unsafe impl wavltree2::Linked for AddressSpaceRegion {
    /// Any heap-allocated type that owns an element may be used.
    ///
    /// An element *must not* move while part of an intrusive data
    /// structure. In many cases, `Pin` may be used to enforce this.
    type Handle = Pin<Box<Self>>; // TODO better handle type

    type Key = VirtualAddress;

    /// Convert an owned `Handle` into a raw pointer
    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        // Safety: wavltree treats the ptr as pinned
        unsafe { NonNull::from(Box::leak(Pin::into_inner_unchecked(handle))) }
    }

    /// Convert a raw pointer back into an owned `Handle`.
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // Safety: `NonNull` *must* be constructed from a pinned reference
        // which the tree implementation upholds.
        unsafe { Pin::new_unchecked(Box::from_raw(ptr.as_ptr())) }
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<wavltree2::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }

    fn get_key(&self) -> &Self::Key {
        &self.range.start
    }

    fn after_insert(self: Pin<&mut Self>) {
        debug_assert_eq!(self.subtree_range.start, self.range.start);
        debug_assert_eq!(self.subtree_range.end, self.range.end);
        debug_assert_eq!(self.max_gap, 0);
        Self::propagate_update_to_parent(self.links.parent());
    }

    fn after_remove(self: Pin<&mut Self>, parent: Option<NonNull<Self>>) {
        Self::propagate_update_to_parent(parent);
    }

    fn after_rotate(
        self: Pin<&mut Self>,
        parent: NonNull<Self>,
        sibling: Option<NonNull<Self>>,
        lr_child: Option<NonNull<Self>>,
        side: wavltree2::Side,
    ) {
        let this = self.project();
        // Safety: caller ensures ptr is valid
        let _parent = unsafe { parent.as_ref() };

        this.subtree_range.start = _parent.subtree_range.start;
        this.subtree_range.end = _parent.subtree_range.end;
        *this.max_gap = _parent.max_gap;

        if side == wavltree2::Side::Left {
            Self::update_gap_metadata(parent, sibling, lr_child);
        } else {
            Self::update_gap_metadata(parent, lr_child, sibling);
        }
    }
}
