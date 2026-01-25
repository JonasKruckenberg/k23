use alloc::alloc::Global;
use core::alloc::{AllocError, Allocator};
use core::marker::PhantomData;
use core::ops::{Bound, Deref};
use core::ptr::NonNull;
use core::{hint, mem, ops};

use crate::int::RangeTreeInteger;
use crate::node::{NodePos, NodeRef, pos, LeafNodePayload};
use crate::stack::Height;
use crate::{Iter, IterMut, RangeTree, RangeTreeIndex};

struct RawCursor<I: RangeTreeIndex, V, A: Allocator, Ref: Deref<Target = RangeTree<I, V, A>>> {
    tree: Ref,
    stack: <I::Int as RangeTreeInteger>::Stack,
}

impl<I: RangeTreeIndex, V, A: Allocator, Ref: Deref<Target = RangeTree<I, V, A>>>
    RawCursor<I, V, A, Ref>
{
    #[inline]
    fn seek(&mut self, search: <I::Int as RangeTreeInteger>::Raw) {
        // Go down the tree, at each internal node selecting the first sub-tree
        // with range greater than or equal to the search range. This sub-tree will
        // only contain ranges less than or equal to its range.
        let mut height = self.tree.height;
        let mut node = self.tree.root;
        while let Some(down) = height.down() {
            let pivots = unsafe { node.pivots(&self.tree.internal) };
            let pos = unsafe { I::Int::search(pivots, search) };
            self.stack[height] = (node, pos);
            node = unsafe { node.child(pos, &self.tree.internal).assume_init_read() };
            height = down;
        }

        // Select the first leaf element with range greater than or equal to the
        // search.
        let pivots = unsafe { node.pivots(&self.tree.leaf) };
        let pos = unsafe { I::Int::search(pivots, search) };
        self.stack[height] = (node, pos);
    }

    /// Returns `true` if the cursor points to the end of the tree.
    #[inline]
    fn is_end(&self) -> bool {
        let (node, pos) = self.stack[Height::LEAF];
        let pivot = unsafe { node.pivot(pos, &self.tree.leaf) };
        pivot == I::Int::MAX
    }

    #[inline]
    fn entry(&self) -> Option<(I, NonNull<LeafNodePayload<I::Int, V>>)> {
        let (node, pos) = self.stack[Height::LEAF];
        let pivot = unsafe { I::Int::from_raw(node.pivot(pos, &self.tree.leaf))? };
        let payload = unsafe { node.payloads_ptr(&self.tree.leaf).add(pos.index()) };
        Some((I::from_int(pivot), payload.cast()))
    }

    /// Advances the cursor to the next element in the tree.
    ///
    /// # Panics
    ///
    /// Panics if the cursor is pointing to the end of the tree.
    #[inline]
    fn next(&mut self) {
        assert!(!self.is_end(), "called next() on cursor already at end");

        // Increment the position in the leaf node.
        let (node, pos) = self.stack[Height::LEAF];
        debug_assert_ne!(unsafe { node.pivot(pos, &self.tree.leaf) }, I::Int::MAX);
        let pos = unsafe { pos.next() };
        self.stack[Height::LEAF].1 = pos;

        // If we reached the end of the leaf then we need to go up the tree to
        // find the next leaf node.
        if unsafe { node.pivot(pos, &self.tree.leaf) } == I::Int::MAX {
            self.next_leaf_node();
        }

        #[cfg(debug_assertions)]
        self.assert_valid();
    }

    /// Advances the cursor to the previous element in the tree.
    ///
    /// If the cursor is already at the first element of the tree then this
    /// method returns `false` and the cursor position is not moved.
    #[inline]
    fn prev(&mut self) -> bool {
        // If we are at the start of the leaf then we need to go up the tree to
        // find the previous leaf node.
        let (_node, pos) = self.stack[Height::LEAF];
        if pos.index() == 0 {
            return self.prev_leaf_node();
        }

        // Decrement the position in the leaf node.
        let pos = unsafe { pos.prev() };
        self.stack[Height::LEAF].1 = pos;

        #[cfg(debug_assertions)]
        self.assert_valid();

        true
    }

    /// Advances the cursor to the start of the next leaf node.
    ///
    /// Leaves the cursor unmodified if this is the last leaf node of the tree.
    #[inline]
    fn next_leaf_node(&mut self) {
        let mut height = Height::LEAF;
        let mut node = loop {
            // If we reached the top of the tree then it means we are on the
            // last entry at all levels of the tree. We've reached the end of
            // the tree and can leave the cursor pointing on an `Int::MAX` range
            // to indicate that.
            let Some(up) = height.up(self.tree.height) else {
                return;
            };

            // The last element of an internal node has a range of `Int::MAX`. If
            // we are not at the last element then we can advance to the next
            // sub-tree and go down that one.
            let (node, pos) = &mut self.stack[up];
            if unsafe { node.pivot(*pos, &self.tree.internal) } != I::Int::MAX {
                *pos = unsafe { pos.next() };
                let node = unsafe { node.child(*pos, &self.tree.internal).assume_init_read() };
                break node;
            }

            // If we reached the end of an internal node, go up to the next
            // level to find a sub-tree to go down.
            height = up;
        };

        // We found a sub-tree, now go down all the way to a leaf node. Since
        // these nodes are guaranteeed to be at least half full we can safely
        // read the first element.
        while let Some(down) = height.down() {
            self.stack[height] = (node, pos!(0));
            node = unsafe { node.child(pos!(0), &self.tree.internal).assume_init_read() };
            height = down;
        }
        self.stack[Height::LEAF] = (node, pos!(0));

        // The tree invariants guarantee that leaf nodes are always at least
        // half full, except if this is the root node. However this can't be the
        // root node since there is more than one node.
        unsafe {
            hint::assert_unchecked(node.pivot(pos!(0), &self.tree.leaf) != I::Int::MAX);
        }
    }

    /// Advances the cursor to the end of the previous leaf node.
    ///
    /// Returns `false` and leaves the cursor unmodified if this is the first
    /// leaf node of the tree.
    #[inline]
    fn prev_leaf_node(&mut self) -> bool {
        let mut height = Height::LEAF;
        let mut node = loop {
            // If we reached the top of the tree then it means we are on the
            // first entry at all levels of the tree. We've reached the start of
            // the tree and can leave the cursor pointing to the start of a
            // leaf node to indicate that.
            let Some(up) = height.up(self.tree.height) else {
                return false;
            };

            // If we are not at the first element then we can advance to the
            // previous sub-tree and go down that one.
            let (node, pos) = &mut self.stack[up];
            if pos.index() != 0 {
                *pos = unsafe { pos.prev() };
                let node = unsafe { node.child(*pos, &self.tree.internal).assume_init_read() };
                break node;
            }

            // If we reached the start of an internal node, go up to the next
            // level to find a sub-tree to go down.
            height = up;
        };

        // We found a sub-tree, now go down all the way to a leaf node. Since
        // these nodes are guaranteeed to be at least half full we can safely
        // read the first element.
        // TODO: Only search high half of the node.
        while let Some(down) = height.down() {
            let pos = unsafe { I::Int::search(node.pivots(&self.tree.internal), I::Int::MAX) };
            self.stack[height] = (node, pos);
            node = unsafe { node.child(pos, &self.tree.internal).assume_init_read() };
            height = down;
        }
        let pos = unsafe { I::Int::search(node.pivots(&self.tree.leaf), I::Int::MAX) };
        self.stack[Height::LEAF] = (node, unsafe { pos.prev() });

        // The tree invariants guarantee that leaf nodes are always at least
        // half full, except if this is the root node. However this can't be the
        // root node since there is more than one node.
        unsafe {
            hint::assert_unchecked(pos.index() != 0);
        }

        true
    }

    fn assert_valid(&self) {
        // The element at each internal level should point to the node lower on
        // the stack.
        let mut height = Height::LEAF;
        while let Some(up) = height.up(self.tree.height) {
            let (node, pos) = self.stack[up];
            let child = self.stack[height].0;

            debug_assert_eq!(
                unsafe { node.child(pos, &self.tree.internal).assume_init_read() },
                child
            );

            height = up;
        }

        // If the leaf node points to an `Int::MAX` range then so must all
        // internal nodes.
        let (node, pos) = self.stack[Height::LEAF];
        if unsafe { node.pivot(pos, &self.tree.leaf) } == I::Int::MAX {
            let mut height = Height::LEAF;
            while let Some(up) = height.up(self.tree.height) {
                let (node, pos) = self.stack[up];
                assert_eq!(unsafe { node.pivot(pos, &self.tree.internal) }, I::Int::MAX);
                height = up;
            }
        }

        assert_eq!(self.stack[self.tree.height].0, self.tree.root);
    }
}

impl<I: RangeTreeIndex, V, A: Allocator> RawCursor<I, V, A, &'_ mut RangeTree<I, V, A>> {
    #[inline]
    unsafe fn update_leaf_max_pivot(&mut self, pivot: <I::Int as RangeTreeInteger>::Raw) {
        let mut height = Height::LEAF;
        // This continues recursively as long as the parent sub-tree is the last
        // one in its node, or the root of the tree is reached.
        while let Some(up) = height.up(self.tree.height) {
            let (node, pos) = self.stack[up];

            if unsafe { node.pivot(pos, &self.tree.internal) } != I::Int::MAX {
                unsafe {
                    node.set_pivot(pivot, pos, &mut self.tree.internal);
                }
                break;
            }
            height = up;
        }
    }

    #[inline]
    fn insert<const AFTER: bool>(
        &mut self,
        range: ops::Range<I>,
        value: V,
    ) -> Result<(), AllocError> {
        let range = range.start.to_int()..range.end.to_int();

        let (node, pos) = self.stack[Height::LEAF];

        let insert_pos = if AFTER {
            assert!(
                !self.is_end(),
                "called insert_after() on cursor already at end"
            );
            unsafe { pos.next() }
        } else {
            pos
        };
        let prev_range = unsafe { node.pivot(insert_pos, &self.tree.leaf) };

        // If we are inserting the last range in a node then we need to update
        // the sub-tree max range in the parent.
        if prev_range == I::Int::MAX {
            if AFTER {
                unsafe {
                    self.update_leaf_max_pivot(range.end.to_raw());
                }
            } else {
                // Note that because of the cursor invariants we don't need to
                // update the sub-tree ranges in any parent nodes:
                // - If the cursor is at the end of the tree then all ranges on
                //   the stack have value `Int::MAX` already.
                // - Otherwise the insertion doesn't happen at the end of the
                //   node, so the maximum range doesn't change.
                debug_assert!(self.is_end());
            }
        }

        // Check if this insertion will cause the leaf node to become completely
        // full. Specifically that after insertion the last range will *not* be
        // `Int::MAX`, which violates the node invariant.
        let overflow = unsafe { node.pivot(pos!(I::Int::B - 2), &self.tree.leaf) } != I::Int::MAX;

        // Save the next leaf pointer since it is overwritten by insertion.
        let next_leaf = unsafe { node.next_leaf(&self.tree.leaf) };

        // Insert the new range and value in the leaf. Use a fast path for
        // inserting at the end of a node. This helps with common cases when
        // appending to the end of a tree.
        if prev_range == I::Int::MAX {
            unsafe {
                node.set_pivot(range.end.to_raw(), insert_pos, &mut self.tree.leaf);
                node.payload_mut(insert_pos, &mut self.tree.leaf).write(LeafNodePayload { value, start: range.start });
            }
        } else {
            unsafe {
                node.insert_pivot(
                    range.end.to_raw(),
                    insert_pos,
                    I::Int::B,
                    &mut self.tree.leaf,
                );
                node.insert_payload(LeafNodePayload { value, start: range.start }, insert_pos, I::Int::B, &mut self.tree.leaf);
            }
        }

        // If insertion didn't overflow then we are done.
        if !overflow {
            // Restore next_leaf which will have been overwritten by the insert.
            unsafe {
                node.set_next_leaf(next_leaf, &mut self.tree.leaf);
            }

            #[cfg(debug_assertions)]
            self.assert_valid();

            return Ok(());
        }

        tracing::trace!("leaf {node:?} overflowed, splitting...");

        // At this point the leaf node is completely full and needs to be split
        // to maintain the node invariant.

        // Record the last range of the first half of the node. This will become
        // the range for the left sub-tree in the parent node.
        let mut mid_range = unsafe { node.pivot(pos!(I::Int::B / 2 - 1), &self.tree.leaf) };

        // Allocate a new node and move the second half of the current node to
        // it.
        let new_uninit_node = unsafe { self.tree.leaf.alloc_node(&self.tree.allocator)? };
        let mut new_node = unsafe { node.leaf_split_into(new_uninit_node, &mut self.tree.leaf) };

        // Update the next-leaf pointers for both nodes.
        unsafe {
            new_node.set_next_leaf(next_leaf, &mut self.tree.leaf);
            node.set_next_leaf(Some(new_node), &mut self.tree.leaf);
        }

        // Keep track of where the cursor is in the tree by adjusting the
        // position on the stack if we were in the second half of the node that
        // got split.
        let mut in_right_split = if let Some(new_pos) = pos.split_right_half() {
            self.stack[Height::LEAF] = (new_node, new_pos);
            true
        } else {
            false
        };

        // Propagate the split by inserting the new node in the next level of
        // the tree. This may cause that node to also be split if it gets full.
        let mut height = Height::LEAF;
        while let Some(up) = height.up(self.tree.height) {
            height = up;
            let (node, mut pos) = self.stack[height];

            // The last 2 ranges of leaf nodes are always `Int::MAX` so we can
            // check if an insertion will cause an overflow by looking at
            // whether the range at `B - 3` is `Int::MAX`.
            let overflow =
                unsafe { node.pivot(pos!(I::Int::B - 3), &self.tree.internal) } != I::Int::MAX;

            // The existing range for this sub-tree (max of all ranges in sub-tree)
            // is correct for the second node of the split. Similarly the
            // existing value already points to the first node of the split. So
            // insert the new range before the existing one and the new value
            // after the existing one.
            unsafe {
                node.insert_pivot(mid_range, pos, I::Int::B, &mut self.tree.internal);
                node.insert_child(new_node, pos.next(), I::Int::B, &mut self.tree.internal);
            }

            // If the node below us ended up on the right side of the split,
            // adjust the cursor position to point to the newly inserted node.
            if in_right_split {
                pos = unsafe { pos.next() };
            }
            self.stack[height].1 = pos;

            // If the node is not full then we're done.
            if !overflow {
                #[cfg(debug_assertions)]
                self.assert_valid();

                return Ok(());
            }

            tracing::trace!("internal node {node:?} at height {height:?} overflowed, splitting...");

            // Record the last range of the first half of the node. This will
            // become the range for the left sub-tree in the parent node.
            mid_range = unsafe { node.pivot(pos!(I::Int::B / 2 - 1), &self.tree.internal) };

            // Set the last range of the first half to `Int::MAX` to indicate that
            // it is the last element in this node.
            unsafe {
                node.set_pivot(
                    I::Int::MAX,
                    pos!(I::Int::B / 2 - 1),
                    &mut self.tree.internal,
                );
            }

            // Allocate a new node and move the second half of the current node
            // to it.
            let new_uninit_node = unsafe { self.tree.internal.alloc_node(&self.tree.allocator)? };
            new_node =
                unsafe { node.internal_split_into(new_uninit_node, &mut self.tree.internal) };

            // Keep track of where the cursor is in the tree by adjusting the
            // position on the stack if we were in the second half of the node
            // that got split.
            in_right_split = if let Some(new_pos) = pos.split_right_half() {
                self.stack[height] = (new_node, new_pos);
                true
            } else {
                false
            };
        }

        tracing::trace!("root node {node:?} at height {height:?} overflowed, splitting...");

        // If we reached the root of the tree then we need to add a new level to
        // the tree and create a new root node.
        let new_uninit_root = unsafe { self.tree.internal.alloc_node(&self.tree.allocator)? };

        // The new root only contains 2 elements: the original root node and the
        // newly created split node. The only non-MAX range is the first one which
        // holds the maximum range in the left sub-tree.
        let new_root;
        unsafe {
            new_root = new_uninit_root.init_pivots(&mut self.tree.internal);
            new_root.set_pivot(mid_range, pos!(0), &mut self.tree.internal);
            new_root
                .child_mut(pos!(0), &mut self.tree.internal)
                .write(self.tree.root);
            new_root
                .child_mut(pos!(1), &mut self.tree.internal)
                .write(new_node);
        };
        self.tree.root = new_root;

        // Increment the height of the tree. The `expect` should never fail here
        // since we calculated the maximum possible height for the tree
        // statically as `Height::max`.
        self.tree.height = self
            .tree
            .height
            .up(Height::MAX)
            .expect("exceeded maximum height");

        // Set up the new level in the cursor stack.
        let pos = if in_right_split { pos!(1) } else { pos!(0) };
        self.stack[self.tree.height] = (new_root, pos);

        #[cfg(debug_assertions)]
        self.assert_valid();

        Ok(())
    }

    /// Replaces the range and value of the element at the given position.
    ///
    /// # Panics
    ///
    /// Panics if the cursor is pointing to the end of the tree.
    #[inline]
    fn replace(&mut self, range: ops::Range<I>, value: V) -> (ops::Range<I>, V) {
        let (node, pos) = self.stack[Height::LEAF];
        let old_pivot = unsafe { node.pivot(pos, &self.tree.leaf) };
        let old_pivot =
            I::Int::from_raw(old_pivot).expect("called replace() on cursor already at end");

        // If we are replacing the last range in a node then we need to update the
        // sub-tree max range in the parent.
        unsafe {
            if node.pivot(pos.next(), &self.tree.leaf) == I::Int::MAX {
                self.update_leaf_max_pivot(range.end.to_int().to_raw());
            }
        }

        // Then actually replace the range and value in the leaf node.
        unsafe {
            node.set_pivot(range.end.to_int().to_raw(), pos, &mut self.tree.leaf);
        }
        let old_payload = unsafe {
            mem::replace(
                node.payload_mut(pos, &mut self.tree.leaf).assume_init_mut(),
                LeafNodePayload { value, start: range.start.to_int() },
            )
        };

        (I::from_int(old_payload.start)..I::from_int(old_pivot), old_payload.value)
    }
}

pub struct Cursor<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    raw: RawCursor<I, V, A, &'a RangeTree<I, V, A>>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Cursor<'a, I, V, A> {
    /// Advances the cursor to the next element in the tree.
    ///
    /// # Panics
    ///
    /// Panics if the cursor is pointing to the end of the tree.
    #[inline]
    pub fn next(&mut self) {
        self.raw.next();
    }

    /// Advances the cursor to the previous element in the tree.
    ///
    /// If the cursor is already at the first element of the tree then this
    /// method returns `false` and the cursor position is not moved.
    #[inline]
    pub fn prev(&mut self) -> bool {
        self.raw.prev()
    }

    /// Returns `true` if the cursor points to the end of the tree.
    #[inline]
    pub fn is_end(&self) -> bool {
        self.entry().is_none()
    }

    /// Returns the range of the element that the cursor is currently pointing to,
    /// or `None` if the cursor is pointing to the end of the tree.
    #[inline]
    pub fn range(&self) -> Option<ops::Range<I>> {
        self.entry().map(|(r, _v)| r)
    }

    /// Returns a reference to the value that the cursor is currently
    /// pointing to, or `None` if the cursor is pointing to the end of the tree.
    #[inline]
    pub fn value(&self) -> Option<&V> {
        self.entry().map(|(_k, v)| v)
    }

    /// Returns the range and a reference to the value that the cursor is
    /// currently pointing to, or `None` if the cursor is pointing to the end of
    /// the tree.
    #[inline]
    pub fn entry(&self) -> Option<(ops::Range<I>, &V)> {
        self.raw
            .entry()
            .map(|(end, payload)| {
                let payload = unsafe { payload.as_ref() };
                let start = I::from_int(payload.start);


                (start..end, &payload.value)
            })
    }

    /// Returns an iterator starting a the current element.
    ///
    /// Iterators are more efficient than cursors. Prefer using them if you don't
    /// need reverse iteration or if you don't need to insert or remove elements in
    /// the tree.
    #[inline]
    pub fn iter(&self) -> Iter<'_, I, V, A> {
        let (node, pos) = self.raw.stack[Height::LEAF];
        Iter {
            raw: crate::iter::RawIter {
                node,
                pos,
                _value: PhantomData,
            },
            tree: self.raw.tree,
        }
    }

    /// Returns an iterator starting a the current element.
    ///
    /// Unlike [`CursorMut::iter`] the returned iterator has the same lifetime
    /// as the cursor and consumes the cursor.
    ///
    /// Iterators are more efficient than cursors. Prefer using them if you don't
    /// need reverse iteration or if you don't need to insert or remove elements in
    /// the tree.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn into_iter(self) -> Iter<'a, I, V, A> {
        let (node, pos) = self.raw.stack[Height::LEAF];
        Iter {
            raw: crate::iter::RawIter {
                node,
                pos,
                _value: PhantomData,
            },
            tree: self.raw.tree,
        }
    }
}

pub struct CursorMut<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    raw: RawCursor<I, V, A, &'a mut RangeTree<I, V, A>>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> CursorMut<'a, I, V, A> {
    #[inline]
    pub(crate) unsafe fn uninit(tree: &'a mut RangeTree<I, V, A>) -> Self {
        Self {
            raw: RawCursor {
                tree,
                stack: <I::Int as RangeTreeInteger>::Stack::default(),
            },
        }
    }

    #[inline]
    pub(crate) fn seek(&mut self, search: <I::Int as RangeTreeInteger>::Raw) {
        self.raw.seek(search);
    }

    /// Advances the cursor to the next element in the tree.
    ///
    /// # Panics
    ///
    /// Panics if the cursor is pointing to the end of the tree.
    #[inline]
    pub fn next(&mut self) {
        self.raw.next();
    }

    /// Advances the cursor to the previous element in the tree.
    ///
    /// If the cursor is already at the first element of the tree then this
    /// method returns `false` and the cursor position is not moved.
    #[inline]
    pub fn prev(&mut self) -> bool {
        self.raw.prev()
    }

    /// Returns `true` if the cursor points to the end of the tree.
    #[inline]
    pub fn is_end(&self) -> bool {
        self.entry().is_none()
    }

    /// Returns the range of the element that the cursor is currently pointing to,
    /// or `None` if the cursor is pointing to the end of the tree.
    #[inline]
    pub fn range(&self) -> Option<ops::Range<I>> {
        self.entry().map(|(r, _v)| r)
    }

    /// Returns a reference to the value that the cursor is currently
    /// pointing to, or `None` if the cursor is pointing to the end of the tree.
    #[inline]
    pub fn value(&self) -> Option<&V> {
        self.entry().map(|(_k, v)| v)
    }

    /// Returns a mutable reference to the value that the cursor is currently
    /// pointing to, or `None` if the cursor is pointing to the end of the tree.
    #[inline]
    pub fn value_mut(&mut self) -> Option<&mut V> {
        self.entry_mut().map(|(_k, v)| v)
    }

    /// Returns the range and a reference to the value that the cursor is
    /// currently pointing to, or `None` if the cursor is pointing to the end of
    /// the tree.
    #[inline]
    pub fn entry(&self) -> Option<(ops::Range<I>, &V)> {
        self.raw
            .entry()
            .map(|(end, payload)| {
                let payload = unsafe { payload.as_ref() };
                let start = I::from_int(payload.start);


                (start..end, &payload.value)
            })
    }

    /// Returns the range and a mutable reference to the value that the cursor is
    /// currently pointing to, or `None` if the cursor is pointing to the end of
    /// the tree.
    #[inline]
    pub fn entry_mut(&mut self) -> Option<(ops::Range<I>, &mut V)> {
        self.raw
            .entry()
            .map(|(end, mut payload)| {
                let payload = unsafe { payload.as_mut() };
                let start = I::from_int(payload.start);


                (start..end, &mut payload.value)
            })
    }

    /// Returns an iterator starting a the current element.
    ///
    /// Iterators are more efficient than cursors. Prefer using them if you don't
    /// need reverse iteration or if you don't need to insert or remove elements in
    /// the tree.
    #[inline]
    pub fn iter(&self) -> Iter<'_, I, V, A> {
        let (node, pos) = self.raw.stack[Height::LEAF];
        Iter {
            raw: crate::iter::RawIter {
                node,
                pos,
                _value: PhantomData,
            },
            tree: self.raw.tree,
        }
    }

    /// Returns a mutable iterator starting a the current element.
    ///
    /// Iterators are more efficient than cursors. Prefer using them if you don't
    /// need reverse iteration or if you don't need to insert or remove elements in
    /// the tree.
    #[inline]
    pub fn iter_mut(&mut self) -> IterMut<'_, I, V, A> {
        let (node, pos) = self.raw.stack[Height::LEAF];
        IterMut {
            raw: crate::iter::RawIter {
                node,
                pos,
                _value: PhantomData,
            },
            tree: self.raw.tree,
        }
    }

    /// Returns an iterator starting a the current element.
    ///
    /// Unlike [`CursorMut::iter`] the returned iterator has the same lifetime
    /// as the cursor and consumes the cursor.
    ///
    /// Iterators are more efficient than cursors. Prefer using them if you don't
    /// need reverse iteration or if you don't need to insert or remove elements in
    /// the tree.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn into_iter(self) -> Iter<'a, I, V, A> {
        let (node, pos) = self.raw.stack[Height::LEAF];
        Iter {
            raw: crate::iter::RawIter {
                node,
                pos,
                _value: PhantomData,
            },
            tree: self.raw.tree,
        }
    }

    /// Returns a mutable iterator starting a the current element.
    ///
    /// Unlike [`CursorMut::iter_mut`] the returned iterator has the same lifetime
    /// as the cursor and consumes the cursor.
    ///
    /// Iterators are more efficient than cursors. Prefer using them if you don't
    /// need reverse iteration or if you don't need to insert or remove elements in
    /// the tree.
    #[inline]
    pub fn into_iter_mut(self) -> IterMut<'a, I, V, A> {
        let (node, pos) = self.raw.stack[Height::LEAF];
        IterMut {
            raw: crate::iter::RawIter {
                node,
                pos,
                _value: PhantomData,
            },
            tree: self.raw.tree,
        }
    }

    /// Inserts `range` and `value` before the element that the cursor is
    /// currently pointing to.
    ///
    /// After insertion the cursor will be pointing to the newly inserted
    /// element.
    ///
    /// If the cursor is pointing to the end of the tree then this inserts the
    /// new element at the end of the tree after all other elements.
    ///
    /// It is the user's responsibility to ensure that inserting `range` at this
    /// position does not violate the invariant that all ranges must be in sorted
    /// order in the tree. Violating this invariant is safe but may cause
    /// other operations to return incorrect results or panic.
    #[inline]
    pub fn insert_before(&mut self, range: ops::Range<I>, value: V) -> Result<(), AllocError> {
        self.raw.insert::<false>(range, value)
    }

    /// Inserts `range` and `value` after the element that the cursor is
    /// currently pointing to.
    ///
    /// After insertion the cursor will still be pointing to the same element as
    /// before the insertion.
    ///
    /// It is the user's responsibility to ensure that inserting `range` at this
    /// position does not violate the invariant that all ranges must be in sorted
    /// order in the tree. Violating this invariant is safe but may cause
    /// other operations to return incorrect results or panic.
    ///
    /// # Panics
    ///
    /// Panics if the cursor is pointing to the end of the tree.
    #[inline]
    pub fn insert_after(&mut self, range: ops::Range<I>, value: V) -> Result<(), AllocError> {
        self.raw.insert::<true>(range, value)
    }

    /// Replaces the range and value of the element that the cursor is currently
    /// pointing to and returns the previous range and value.
    ///
    /// It is the user's responsibility to ensure that inserting `range` at this
    /// position does not violate the invariant that all ranges must be in sorted
    /// order in the tree. Violating this invariant is safe but may cause
    /// other operations to return incorrect results or panic.
    ///
    /// # Panics
    ///
    /// Panics if the cursor is pointing to the end of the tree.
    #[inline]
    pub fn replace(&mut self, range: ops::Range<I>, value: V) -> (ops::Range<I>, V) {
        self.raw.replace(range, value)
    }
}

impl<I: RangeTreeIndex, V, A: Allocator> RangeTree<I, V, A> {
    /// Returns a [`RawCursor`] pointing at the first element of the tree.
    #[inline]
    fn raw_cursor<Ref: Deref<Target = Self>>(tree: Ref) -> RawCursor<I, V, A, Ref> {
        let mut stack = <I::Int as RangeTreeInteger>::Stack::default();

        // Go down the tree, at each internal node selecting the first sub-tree.
        let mut height = tree.height;
        let mut node = tree.root;
        while let Some(down) = height.down() {
            stack[height] = (node, pos!(0));
            node = unsafe { node.child(pos!(0), &tree.internal).assume_init_read() };
            height = down;
        }

        // The first leaf node is always the left-most leaf on the tree and is
        // never deleted.
        debug_assert_eq!(node, NodeRef::ZERO);
        stack[height] = (NodeRef::ZERO, NodePos::ZERO);
        RawCursor { stack, tree }
    }

    /// Returns a [`RawCursor`] pointing at the first element with range greater
    /// than `bound`.
    #[inline]
    fn raw_cursor_at<Ref: Deref<Target = Self>>(
        tree: Ref,
        search: <I::Int as RangeTreeInteger>::Raw,
    ) -> RawCursor<I, V, A, Ref> {
        let stack = <I::Int as RangeTreeInteger>::Stack::default();
        let mut cursor = RawCursor { stack, tree };
        cursor.seek(search);
        cursor
    }

    /// Returns a [`Cursor`] pointing at the first element of the tree.
    #[inline]
    pub fn cursor(&self) -> Cursor<'_, I, V, A> {
        let raw = Self::raw_cursor(self);
        Cursor { raw }
    }

    /// Returns a [`Cursor`] pointing at the first element with range greater
    /// than `bound`.
    #[inline]
    pub fn cursor_at(&self, bound: Bound<I>) -> Cursor<'_, I, V, A> {
        let search = match bound {
            Bound::Included(search) => search.to_int().to_raw(),
            Bound::Excluded(search) => I::Int::increment(search.to_int().to_raw()),
            Bound::Unbounded => I::Int::MAX,
        };
        let raw = Self::raw_cursor_at(self, search);
        Cursor { raw }
    }

    /// Returns a [`CursorMut`] pointing at the first element of the tree.
    #[inline]
    pub fn cursor_mut(&mut self) -> CursorMut<'_, I, V, A> {
        let raw = Self::raw_cursor(self);
        CursorMut { raw }
    }

    /// Returns a [`CursorMut`] pointing at the first element with range greater
    /// than `bound`.
    #[inline]
    pub fn cursor_mut_at(&mut self, bound: Bound<I>) -> CursorMut<'_, I, V, A> {
        let search = match bound {
            Bound::Included(search) => search.to_int().to_raw(),
            Bound::Excluded(search) => I::Int::increment(search.to_int().to_raw()),
            Bound::Unbounded => I::Int::MAX,
        };
        let raw = Self::raw_cursor_at(self, search);
        CursorMut { raw }
    }
}
