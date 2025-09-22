// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use core::alloc::Layout;
use core::mem::offset_of;
use core::ops::Range;
use core::pin::Pin;
use core::ptr::NonNull;
use core::{cmp, mem};

use pin_project::pin_project;

use crate::{AccessRules, VirtualAddress};

#[pin_project(!Unpin)]
#[derive(Debug)]
pub struct AddressSpaceRegion {
    range: Range<VirtualAddress>,
    access_rules: AccessRules,
    #[cfg(debug_assertions)]
    layout: Layout,

    /// The address range covered by this region and its WAVL tree subtree, used when allocating new regions
    subtree_range: Range<VirtualAddress>,
    /// The largest gap in this subtree, used when allocating new regions
    max_gap: usize,

    /// Links to other regions in the WAVL tree
    #[pin]
    links: wavltree::Links<AddressSpaceRegion>,
}

impl AddressSpaceRegion {
    #[cfg_attr(not(test), allow(unused, reason = "used by tests and later changes"))]
    pub const fn new(
        spot: VirtualAddress,
        access_rules: AccessRules,
        #[cfg(debug_assertions)] layout: Layout,
    ) -> Self {
        Self {
            range: spot..spot.checked_add(layout.size()).unwrap(),
            access_rules,
            #[cfg(debug_assertions)]
            layout,

            max_gap: 0,
            subtree_range: spot..spot.checked_add(layout.size()).unwrap(),
            links: wavltree::Links::new(),
        }
    }

    pub const fn range(&self) -> &Range<VirtualAddress> {
        &self.range
    }

    pub const fn subtree_range(&self) -> &Range<VirtualAddress> {
        &self.subtree_range
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
        // Safety: we have to trust the intrusive tree implementation here
        Some(unsafe { self.links.left()?.as_ref() })
    }

    /// Returns the right child node in the search tree of regions, used during gap-searching.
    pub fn right_child(&self) -> Option<&Self> {
        // Safety: we have to trust the intrusive tree implementation here
        Some(unsafe { self.links.right()?.as_ref() })
    }

    /// Returns the parent node in the search tree of regions, used during gap-searching.
    pub fn parent(&self) -> Option<&Self> {
        // Safety: we have to trust the intrusive tree implementation here
        Some(unsafe { self.links.parent()?.as_ref() })
    }

    /// Update the gap search metadata of this region. This method is called in the [`wavltree::Linked`]
    /// implementation below after each tree mutation that impacted this node or its subtree in some way
    /// (insertion, rotation, deletion).
    ///
    /// Returns `true` if this nodes metadata changed.
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

        // Safety: we have to trust the intrusive tree implementation
        let node = unsafe { node.as_mut() };
        let mut left_max_gap = 0;
        let mut right_max_gap = 0;

        // recalculate the subtree_range start
        let old_subtree_range_start = if let Some(left) = left {
            // Safety: we have to trust the intrusive tree implementation
            let left = unsafe { left.as_ref() };
            let left_gap = gap(left.subtree_range.end, node.range.start);
            left_max_gap = cmp::max(left_gap, left.max_gap);
            mem::replace(&mut node.subtree_range.start, left.subtree_range.start)
        } else {
            mem::replace(&mut node.subtree_range.start, node.range.start)
        };

        // recalculate the subtree range end
        let old_subtree_range_end = if let Some(right) = right {
            // Safety: we have to trust the intrusive tree implementation
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
    fn propagate_update_to_parent(mut maybe_node: Option<NonNull<Self>>) {
        while let Some(node) = maybe_node {
            // Safety: we have to trust the intrusive tree implementation
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

// Safety: the pinning and !Unpin requirements are enforced by the `#[pin_project(!Unpin)]` attribute
// of the `AddressSpaceRegion`. see above.
unsafe impl wavltree::Linked for AddressSpaceRegion {
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

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<wavltree::Links<Self>> {
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
        side: wavltree::Side,
    ) {
        let this = self.project();
        // Safety: caller ensures ptr is valid
        let _parent = unsafe { parent.as_ref() };

        this.subtree_range.start = _parent.subtree_range.start;
        this.subtree_range.end = _parent.subtree_range.end;
        *this.max_gap = _parent.max_gap;

        if side == wavltree::Side::Left {
            Self::update_gap_metadata(parent, sibling, lr_child);
        } else {
            Self::update_gap_metadata(parent, lr_child, sibling);
        }
    }
}

#[cfg(test)]
mod tests {
    use core::alloc::Layout;

    use wavltree::WAVLTree;

    use super::*;
    use crate::{AccessRules, VirtualAddress};

    #[test]
    fn region_insert_higher() {
        let mut tree: WAVLTree<AddressSpaceRegion> = WAVLTree::new();

        tree.insert(Box::pin(AddressSpaceRegion::new(
            VirtualAddress::new(0),
            AccessRules::new().with(AccessRules::READ, true),
            Layout::from_size_align(4 * 4096, 4096).unwrap(),
        )));

        tree.insert(Box::pin(AddressSpaceRegion::new(
            VirtualAddress::new(5 * 4096),
            AccessRules::new().with(AccessRules::READ, true),
            Layout::from_size_align(4 * 4096, 4096).unwrap(),
        )));

        let a = tree.find(&VirtualAddress::new(0)).get().unwrap();
        let b = tree.find(&VirtualAddress::new(5 * 4096)).get().unwrap();

        // we expect the *first* region to be the parent and therefore hold the big range
        assert_eq!(
            a.subtree_range,
            VirtualAddress::new(0)..VirtualAddress::new(9 * 4096)
        );
        assert_eq!(a.max_gap, 4096);

        // the *second* subtree should just hold its own range
        assert_eq!(
            b.subtree_range,
            VirtualAddress::new(5 * 4096)..VirtualAddress::new(9 * 4096)
        );
        assert_eq!(b.range, b.subtree_range);
        assert_eq!(b.max_gap, 0);
    }

    #[test]
    fn region_insert_lower() {
        let mut tree: WAVLTree<AddressSpaceRegion> = WAVLTree::new();

        tree.insert(Box::pin(AddressSpaceRegion::new(
            VirtualAddress::new(5 * 4096),
            AccessRules::new().with(AccessRules::READ, true),
            Layout::from_size_align(4 * 4096, 4096).unwrap(),
        )));
        tree.insert(Box::pin(AddressSpaceRegion::new(
            VirtualAddress::new(0),
            AccessRules::new().with(AccessRules::READ, true),
            Layout::from_size_align(4 * 4096, 4096).unwrap(),
        )));

        let a = tree.find(&VirtualAddress::new(0)).get().unwrap();
        let b = tree.find(&VirtualAddress::new(5 * 4096)).get().unwrap();

        // the *second* subtree should just hold its own range
        assert_eq!(
            b.subtree_range,
            VirtualAddress::new(0)..VirtualAddress::new(9 * 4096)
        );
        assert_eq!(b.max_gap, 4096);

        // we expect the *first* region to be the parent and therefore hold the big range
        assert_eq!(
            a.subtree_range,
            VirtualAddress::new(0)..VirtualAddress::new(4 * 4096)
        );
        assert_eq!(a.range, a.subtree_range);
        assert_eq!(a.max_gap, 0);
    }

    #[test]
    fn region_updates_after_rotates() {
        let mut tree: WAVLTree<AddressSpaceRegion> = WAVLTree::new();
        let mut addr = VirtualAddress::new(0);

        for _ in 0..10 {
            tree.insert(Box::pin(AddressSpaceRegion::new(
                addr,
                AccessRules::new().with(AccessRules::READ, true),
                Layout::from_size_align(4096, 4096).unwrap(),
            )));
            addr = addr.checked_add(11 * 4096).unwrap();
        }

        for region in tree.iter() {
            if region.left_child().is_some() || region.right_child().is_some() {
                // all gaps are the same size (10 pages)
                assert_eq!(region.max_gap, 10 * 4096);
            } else {
                // regions without children should not have a max_gap
                assert_eq!(region.max_gap, 0);
            }

            assert_eq!(calculate_subtree_range(region), region.subtree_range)
        }
    }

    fn calculate_subtree_range(region: &AddressSpaceRegion) -> Range<VirtualAddress> {
        let mut range = region.range.clone();

        if let Some(left) = region.left_child().map(calculate_subtree_range) {
            range.start = cmp::min(range.start, left.start);
        }
        if let Some(right) = region.right_child().map(calculate_subtree_range) {
            range.end = cmp::max(range.end, right.end);
        }

        range
    }
}
