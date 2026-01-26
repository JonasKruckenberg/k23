//! Cursor types for tree traversal and manipulation.
//!
//! This module contains the implementation of the core B+ Tree algorithms.

use alloc::alloc::{Allocator, Global};
use core::alloc::AllocError;
use core::ops::{Bound, Deref};
use core::ptr::NonNull;
use core::{hint, mem, ops};

use crate::int::{int_from_pivot, pivot_from_int};
use crate::node::{NodePool, NodePos, NodeRef};
use crate::stack::Height;
use crate::{Iter, IterMut, RangeTree, RangeTreeIndex, RangeTreeInteger};

const PLACEHOLDER_MAX_GAP: u32 = 0;

/// Common base for mutable and immutable cursors.
pub(crate) struct RawCursor<
    I: RangeTreeIndex,
    V,
    A: Allocator,
    Ref: Deref<Target = RangeTree<I, V, A>>,
> {
    /// Array of node and position pairs for each level of the tree.
    ///
    /// Invariants:
    /// - Only levels between 0 and `tree.height` are valid.
    /// - Positions in internal nodes must match the node on the next level of
    ///   the stack. This implies that positions in internal nodes must be
    ///   in-bounds.
    /// - Positions in leaf nodes must point to a valid entry *except* if the
    ///   cursor has reached the end of the tree, in which case it must point to
    ///   the first `Int::MAX` pivot in the node.
    ///
    /// These invariants may be temporarily violated during cursor operations.
    stack: <I::Int as RangeTreeInteger>::Stack,

    /// Reference to the underlying `BTree`.
    ///
    /// This is either a mutable or immutable reference depending on the type of
    /// cursor.
    tree: Ref,
}

impl<I: RangeTreeIndex, V, A: Allocator, Ref: Deref<Target = RangeTree<I, V, A>>> Clone
    for RawCursor<I, V, A, Ref>
where
    Ref: Clone,
{
    #[inline]
    fn clone(&self) -> Self {
        Self {
            stack: self.stack.clone(),
            tree: self.tree.clone(),
        }
    }
}

impl<I: RangeTreeIndex, V, A: Allocator, Ref: Deref<Target = RangeTree<I, V, A>>>
    RawCursor<I, V, A, Ref>
{
    /// Initializes a cursor to point to the given pivot.
    #[inline]
    fn seek(&mut self, pivot: <I::Int as RangeTreeInteger>::Raw) {
        // Go down the tree, at each internal node selecting the first sub-tree
        // with pivot greater than or equal to the search pivot. This sub-tree will
        // only contain pivots less than or equal to its pivot.
        let mut height = self.tree.height;
        let mut node = self.tree.root;
        while let Some(down) = height.down() {
            let pivots = unsafe { node.pivots(&self.tree.internal) };
            let pos = unsafe { I::Int::search(pivots, pivot) };
            self.stack[height] = (node, pos);
            node = unsafe { node.value(pos, &self.tree.internal).assume_init_read().0 };
            height = down;
        }

        // Select the first leaf element with pivot greater than or equal to the
        // search pivot.
        let pivots = unsafe { node.pivots(&self.tree.leaf) };
        let pos = unsafe { I::Int::search(pivots, pivot) };
        self.stack[height] = (node, pos);
    }

    /// Helper function to check that cursor invariants are maintained.
    #[inline]
    fn assert_valid(&self) {
        // The element at each internal level should point to the node lower on
        // the stack.
        let mut height = Height::LEAF;
        while let Some(up) = height.up(self.tree.height) {
            let (node, pos) = self.stack[up];
            let child = self.stack[height].0;
            debug_assert_eq!(
                unsafe { node.value(pos, &self.tree.internal).assume_init_read().0 },
                child
            );
            height = up;
        }

        // If the leaf node points to an `Int::MAX` pivot then so must all
        // internal nodes.
        let (node, pos) = self.stack[Height::LEAF];
        if unsafe { node.pivot(pos, &self.tree.leaf) } == I::Int::MAX {
            let mut height = Height::LEAF;
            while let Some(up) = height.up(self.tree.height) {
                let (node, pos) = self.stack[up];
                debug_assert_eq!(unsafe { node.pivot(pos, &self.tree.internal) }, I::Int::MAX);
                height = up;
            }
        }

        debug_assert_eq!(self.stack[self.tree.height].0, self.tree.root);
    }

    /// Returns `true` if the cursor points to the end of the tree.
    #[inline]
    fn is_end(&self) -> bool {
        self.entry().is_none()
    }

    /// Returns the pivot and a reference to the pivot and value at the cursor
    /// position, or `None` if the cursor is pointing to the end of the tree.
    #[inline]
    fn entry(&self) -> Option<(I, NonNull<(I, V)>)> {
        let (node, pos) = self.stack[Height::LEAF];
        let pivot = unsafe { node.pivot(pos, &self.tree.leaf) };
        let pivot = pivot_from_int(pivot)?;
        let value = unsafe { node.values_ptr(&self.tree.leaf).add(pos.index()) };
        Some((pivot, value.cast()))
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
            // the tree and can leave the cursor pointing on an `Int::MAX` pivot
            // to indicate that.
            let Some(up) = height.up(self.tree.height) else {
                return;
            };

            // The last element of an internal node has a pivot of `Int::MAX`. If
            // we are not at the last element then we can advance to the next
            // sub-tree and go down that one.
            let (node, pos) = &mut self.stack[up];
            if unsafe { node.pivot(*pos, &self.tree.internal) } != I::Int::MAX {
                *pos = unsafe { pos.next() };
                let node = unsafe { node.value(*pos, &self.tree.internal).assume_init_read().0 };
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
            node = unsafe {
                node.value(pos!(0), &self.tree.internal)
                    .assume_init_read()
                    .0
            };
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
                let node = unsafe { node.value(*pos, &self.tree.internal).assume_init_read().0 };
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
            node = unsafe { node.value(pos, &self.tree.internal).assume_init_read().0 };
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

        #[cfg(debug_assertions)]
        self.assert_valid();

        true
    }
}

impl<I: RangeTreeIndex, V, A: Allocator> RawCursor<I, V, A, &'_ mut RangeTree<I, V, A>> {
    /// Propagates the maximum pivot in a leaf node to parent nodes.
    ///
    /// # Safety
    ///
    /// `pivot` must be the largest non-`MAX` pivot in the current leaf node.
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

    /// Common code for `insert_before` and `insert_after`.
    ///
    /// After insertion the leaf position will be unchanged.
    #[inline]
    fn insert<const AFTER: bool>(
        &mut self,
        range: ops::Range<I>,
        value: V,
    ) -> Result<(), AllocError> {
        let pivot = int_from_pivot(range.end);
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
        let prev_pivot = unsafe { node.pivot(insert_pos, &self.tree.leaf) };

        // If we are inserting the last pivot in a node then we need to update
        // the sub-tree max pivot in the parent.
        if prev_pivot == I::Int::MAX {
            if AFTER {
                unsafe {
                    self.update_leaf_max_pivot(pivot);
                }
            } else {
                // Note that because of the cursor invariants we don't need to
                // update the sub-tree pivots in any parent nodes:
                // - If the cursor is at the end of the tree then all pivots on
                //   the stack have value `Int::MAX` already.
                // - Otherwise the insertion doesn't happen at the end of the
                //   node, so the maximum pivot doesn't change.
                debug_assert!(self.is_end());
            }
        }

        // Check if this insertion will cause the leaf node to become completely
        // full. Specifically that after insertion the last pivot will *not* be
        // `Int::MAX`, which violates the node invariant.
        let overflow = unsafe { node.pivot(pos!(I::Int::B - 2), &self.tree.leaf) } != I::Int::MAX;

        // Save the next leaf pointer since it is overwritten by insertion.
        let next_leaf = unsafe { node.next_leaf(&self.tree.leaf) };

        // Insert the new pivot and value in the leaf. Use a fast path for
        // inserting at the end of a node. This helps with common cases when
        // appending to the end of a tree.
        if prev_pivot == I::Int::MAX {
            unsafe {
                node.set_pivot(pivot, insert_pos, &mut self.tree.leaf);
                node.value_mut(insert_pos, &mut self.tree.leaf)
                    .write((range.start, value));
            }
        } else {
            unsafe {
                node.insert_pivot(pivot, insert_pos, I::Int::B, &mut self.tree.leaf);
                node.insert_value(
                    (range.start, value),
                    insert_pos,
                    I::Int::B,
                    &mut self.tree.leaf,
                );
            }
        }

        // If insertion didn't overflow then we are done.
        if !overflow {
            // Restore next_leaf which will have been overwritten by the insert.
            unsafe {
                node.set_next_leaf(next_leaf, &mut self.tree.leaf);
            }
            return Ok(());
        }

        // At this point the leaf node is completely full and needs to be split
        // to maintain the node invariant.

        // Record the last pivot of the first half of the node. This will become
        // the pivot for the left sub-tree in the parent node.
        let mut mid_pivot = unsafe { node.pivot(pos!(I::Int::B / 2 - 1), &self.tree.leaf) };

        // Allocate a new node and move the second half of the current node to
        // it.
        let new_uninit_node = unsafe { self.tree.leaf.alloc_node(&self.tree.alloc)? };
        let mut new_node = unsafe { node.split_into(new_uninit_node, &mut self.tree.leaf) };

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

            // The last 2 pivots of leaf nodes are always `Int::MAX` so we can
            // check if an insertion will cause an overflow by looking at
            // whether the pivot at `B - 3` is `Int::MAX`.
            let overflow =
                unsafe { node.pivot(pos!(I::Int::B - 3), &self.tree.internal) } != I::Int::MAX;

            // The existing pivot for this sub-tree (max of all pivots in sub-tree)
            // is correct for the second node of the split. Similarly the
            // existing value already points to the first node of the split. So
            // insert the new pivot before the existing one and the new value
            // after the existing one.
            unsafe {
                node.insert_pivot(mid_pivot, pos, I::Int::B, &mut self.tree.internal);
                node.insert_value(
                    (new_node, PLACEHOLDER_MAX_GAP),
                    pos.next(),
                    I::Int::B,
                    &mut self.tree.internal,
                );
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

            // Record the last pivot of the first half of the node. This will
            // become the pivot for the left sub-tree in the parent node.
            mid_pivot = unsafe { node.pivot(pos!(I::Int::B / 2 - 1), &self.tree.internal) };

            // Set the last pivot of the first half to `Int::MAX` to indicate that
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
            let new_uninit_node = unsafe { self.tree.internal.alloc_node(&self.tree.alloc)? };
            new_node = unsafe { node.split_into(new_uninit_node, &mut self.tree.internal) };

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

        // If we reached the root of the tree then we need to add a new level to
        // the tree and create a new root node.
        let new_uninit_root = unsafe { self.tree.internal.alloc_node(&self.tree.alloc)? };

        // The new root only contains 2 elements: the original root node and the
        // newly created split node. The only non-MAX pivot is the first one which
        // holds the maximum pivot in the left sub-tree.
        let new_root;
        unsafe {
            new_root = new_uninit_root.init_pivots(&mut self.tree.internal);
            new_root.set_pivot(mid_pivot, pos!(0), &mut self.tree.internal);
            new_root
                .value_mut(pos!(0), &mut self.tree.internal)
                .write((self.tree.root, PLACEHOLDER_MAX_GAP));
            new_root
                .value_mut(pos!(1), &mut self.tree.internal)
                .write((new_node, PLACEHOLDER_MAX_GAP));
        }
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

    /// Replaces the pivot and value of the element at the given position.
    ///
    /// # Panics
    ///
    /// Panics if the cursor is pointing to the end of the tree.
    #[inline]
    fn replace(&mut self, range: ops::Range<I>, value: V) -> (ops::Range<I>, V) {
        let pivot = int_from_pivot(range.end);

        let (node, pos) = self.stack[Height::LEAF];
        let old_pivot = unsafe { node.pivot(pos, &self.tree.leaf) };
        let old_pivot =
            pivot_from_int(old_pivot).expect("called replace() on cursor already at end");

        // If we are replacing the last pivot in a node then we need to update the
        // sub-tree max pivot in the parent.
        unsafe {
            if node.pivot(pos.next(), &self.tree.leaf) == I::Int::MAX {
                self.update_leaf_max_pivot(pivot);
            }
        }

        // Then actually replace the pivot and value in the leaf node.
        unsafe {
            node.set_pivot(pivot, pos, &mut self.tree.leaf);
        }
        let (old_start, old_value) = unsafe {
            mem::replace(
                node.value_mut(pos, &mut self.tree.leaf).assume_init_mut(),
                (range.start, value),
            )
        };

        (old_start..old_pivot, old_value)
    }

    /// Removes the element to the right of the cursor and returns it.
    ///
    /// # Panics
    ///
    /// Panics if the cursor is pointing to the end of the tree.
    #[inline]
    fn remove(&mut self) -> (ops::Range<I>, V) {
        let (node, pos) = self.stack[Height::LEAF];

        // Check if this deletion will cause the leaf node to become less than
        // half full. Specifically that after deletion last pivot in the first
        // half will be`Int::MAX`, which violates the node invariant.
        let underflow = unsafe { node.pivot(pos!(I::Int::B / 2), &self.tree.leaf) } == I::Int::MAX;

        // Extract the pivot and value that will be returned by this function.
        let pivot = unsafe {
            pivot_from_int(node.pivot(pos, &self.tree.leaf))
                .expect("called remove() on cursor already at end")
        };
        let (start, value) = unsafe { node.value(pos, &self.tree.leaf).assume_init_read() };

        // Remove the pivot and value from the node.
        unsafe {
            node.remove_pivot(pos, &mut self.tree.leaf);
            node.remove_value(pos, &mut self.tree.leaf);
        }

        // If we removed the last pivot in a node then we need to update the
        // sub-tree max pivot in the parent.
        unsafe {
            if node.pivot(pos, &self.tree.leaf) == I::Int::MAX && self.tree.height != Height::LEAF {
                // Leaf nodes must be at least half full if they are not the
                // root node.
                let new_max = node.pivot(pos.prev(), &self.tree.leaf);
                self.update_leaf_max_pivot(new_max);
            }
        }

        // If the leaf node is now less than half-full, we need to either steal
        // an element from a sibling node or merge it with a sibling to restore
        // the node invariant that it must always be at least half full..
        if underflow {
            // If there is only a single leaf node in the tree then it is
            // allowed to have as little as zero elements and cannot underflow.
            if let Some(up) = Height::LEAF.up(self.tree.height) {
                // `node` is less than half-full, try to restore the invariant
                // by stealing from another node or merging it.
                let up_node = unsafe {
                    self.handle_underflow(Height::LEAF, up, node, true, |tree| &mut tree.leaf)
                };
                if let Some(mut node) = up_node {
                    let mut height = up;
                    loop {
                        if let Some(up) = height.up(self.tree.height) {
                            // Check if this node is less than half full. A
                            // half-full internal node would have the first
                            // `Int::MAX` pivot at `B / 2 - 1`.
                            if unsafe { node.pivot(pos!(I::Int::B / 2 - 2), &self.tree.internal) }
                                == I::Int::MAX
                            {
                                // `node` is less than half-full, try to restore
                                // the invariant by stealing from another node
                                // or merging it.
                                if let Some(up_node) = unsafe {
                                    self.handle_underflow(height, up, node, false, |tree| {
                                        &mut tree.internal
                                    })
                                } {
                                    // If the underflow was resolved by merging
                                    // then the parent node could have become
                                    // less than half-full itself. Loop back
                                    // and do the same with the parent.
                                    node = up_node;
                                    height = up;
                                    continue;
                                }
                            }
                        } else {
                            // We've reached the root node. If it only has a
                            // single element then we can pop a level off the
                            // tree and free the old root node.
                            debug_assert_eq!(node, self.tree.root);
                            if unsafe { node.pivot(pos!(0), &self.tree.internal) } == I::Int::MAX {
                                unsafe {
                                    self.tree.root = node
                                        .value(pos!(0), &self.tree.internal)
                                        .assume_init_read()
                                        .0;
                                }
                                unsafe {
                                    self.tree.internal.free_node(node);
                                }
                                self.tree.height = height.down().unwrap();
                            }
                        }
                        break;
                    }
                }
            }
        }

        // If we ended up at the end of a leaf node due to the deletion, advance
        // the cursor to the next element.
        if self.is_end() {
            self.next_leaf_node();
        }

        self.assert_valid();

        (start..pivot, value)
    }

    /// Given `child` which is less than half full, restores the invariant that
    /// nodes must be at least half full by stealing an element from a sibling
    /// or merging `child` with a sibling node.
    ///
    /// If this is resolved through merging, this function returns a `NodeRef`
    /// to the parent of `child` which may now be under-filled.
    ///
    /// # Safety
    ///
    /// - `up` is the level above the one containing `child`.
    /// - `child` must have exact `B / 2 - 1` elements.
    /// - `child_is_leaf` indicates whether `child` is a leaf node and
    ///   `child_pool` returns a reference to the appropriate `NodePool`.
    #[inline]
    unsafe fn handle_underflow<ChildValue>(
        &mut self,
        height: Height<I::Int>,
        up: Height<I::Int>,
        child: NodeRef,
        child_is_leaf: bool,
        child_pool: impl Fn(&mut RangeTree<I, V, A>) -> &mut NodePool<I::Int, ChildValue>,
    ) -> Option<NodeRef> {
        // The child must have exactly `B / 2 - 1` elements.
        debug_assert_eq!(
            unsafe {
                if child_is_leaf {
                    child.leaf_end(&self.tree.leaf).index()
                } else {
                    child.internal_end(&self.tree.internal).index()
                }
            },
            I::Int::B / 2 - 1
        );

        // Check if the child is the last sub-tree in its parent. The last
        // sub-tree always has a pivot of `Int::MAX`.
        let (node, pos) = self.stack[up];
        debug_assert_eq!(
            unsafe { node.value(pos, &self.tree.internal).assume_init_read().0 },
            child
        );
        let child_sutree_max = unsafe { node.pivot(pos, &self.tree.internal) };

        // We now need to select a sibling node to steal from or merge with.
        // Prefer using the next sub-tree as a sibling since it has a more
        // efficient code path.
        if child_sutree_max != I::Int::MAX {
            let (sibling, _) = unsafe {
                node.value(pos.next(), &self.tree.internal)
                    .assume_init_read()
            };

            // We can steal from the sibling if it is more than half-full.
            let can_steal = unsafe {
                sibling.pivot(
                    if child_is_leaf {
                        pos!(I::Int::B / 2)
                    } else {
                        pos!(I::Int::B / 2 - 1)
                    },
                    child_pool(self.tree),
                )
            } != I::Int::MAX;

            if can_steal {
                unsafe {
                    // Remove the first pivot/value from the sibling.
                    let pivot = sibling.pivot(pos!(0), child_pool(self.tree));
                    let value = sibling
                        .value(pos!(0), child_pool(self.tree))
                        .assume_init_read();
                    sibling.remove_pivot(pos!(0), child_pool(self.tree));
                    sibling.remove_value(pos!(0), child_pool(self.tree));

                    if child_is_leaf {
                        // If the child is a leaf node then we can just insert
                        // the pivot/value at `B / 2 - 1` since we know the child
                        // currently has exactly that many elements.
                        child.set_pivot(pivot, pos!(I::Int::B / 2 - 1), child_pool(self.tree));
                        child
                            .value_mut(pos!(I::Int::B / 2 - 1), child_pool(self.tree))
                            .write(value);
                    } else {
                        // If the child is an internal node then we need to set
                        // the pivot for the *previous* sub-tree (which is
                        // currently `Int::MAX`) to `child_sutree_max` which is
                        // the maximum pivot for that sub-tree before the steal.
                        child.set_pivot(
                            child_sutree_max,
                            pos!(I::Int::B / 2 - 2),
                            child_pool(self.tree),
                        );
                        child
                            .value_mut(pos!(I::Int::B / 2 - 1), child_pool(self.tree))
                            .write(value);
                    }

                    // The steal has caused the largest pivot in `child` to
                    // increase (since we appended to its end). Update the pivot
                    // for this sub-tree in the parent to the pivot for the stolen
                    // element.
                    node.set_pivot(pivot, pos, &mut self.tree.internal);
                }

                // Stealing can't cause recursive underflows.
                None
            } else {
                unsafe {
                    // The sibling has exactly `B / 2` elements, move those to
                    // the end of the child which has exactly `B / 2 - 1`
                    // elements. This results in a full node with the maximum of
                    // `B - 1` elements.
                    child.merge_from(
                        sibling,
                        pos!(I::Int::B / 2 - 1),
                        I::Int::B / 2,
                        child_pool(self.tree),
                    );

                    // If this is an internal node then we need to copy the
                    // previous maximum pivot for the child's sub-tree to slot
                    // `B / 2 - 2`  which previously contained MAX.
                    if !child_is_leaf {
                        child.set_pivot(
                            child_sutree_max,
                            pos!(I::Int::B / 2 - 2),
                            child_pool(self.tree),
                        );
                    }

                    // Update the next leaf pointer if this is a leaf node.
                    if child_is_leaf {
                        let next_leaf = sibling.next_leaf(child_pool(self.tree));
                        child.set_next_leaf(next_leaf, child_pool(self.tree));
                    }

                    // The sibling is no longer in the tree, free its node.
                    child_pool(self.tree).free_node(sibling);

                    // Remove the sibling node from its parent. We keep the pivot
                    // of `sibling` and remove that of `child` because the pivot
                    // should hold the maximum pivot in the sub-tree.
                    node.remove_pivot(pos, &mut self.tree.internal);
                    node.remove_value(pos.next(), &mut self.tree.internal);
                }

                // Merging may cause the parent node to become under-sized.
                Some(node)
            }
        } else {
            let (sibling, _) = unsafe {
                node.value(pos.prev(), &self.tree.internal)
                    .assume_init_read()
            };

            // We can steal from the sibling if it is more than half-full.
            let can_steal = unsafe {
                sibling.pivot(
                    if child_is_leaf {
                        pos!(I::Int::B / 2)
                    } else {
                        pos!(I::Int::B / 2 - 1)
                    },
                    child_pool(self.tree),
                )
            } != I::Int::MAX;

            if can_steal {
                unsafe {
                    // Find the position of the last element in the sibling.
                    let sibling_end = if child_is_leaf {
                        sibling.leaf_end(child_pool(self.tree))
                    } else {
                        sibling.internal_end(child_pool(self.tree))
                    };
                    let sibling_last = sibling_end.prev();

                    if child_is_leaf {
                        // If the child is a leaf node then we can just take the
                        // last pivot/value of the sibling and insert it at the
                        // start of the child.
                        //
                        // We use a node size of `B / 2 + 1` so that the
                        // operation becomes a copy of exactly `B / 2` elements.
                        // All elements in the second half of the node are
                        // absent anyways. This also preserves the next leaf
                        // pointer.
                        let pivot = sibling.pivot(sibling_last, child_pool(self.tree));
                        let value = sibling
                            .value(sibling_last, child_pool(self.tree))
                            .assume_init_read();
                        child.insert_pivot(
                            pivot,
                            pos!(0),
                            I::Int::B / 2 + 1,
                            child_pool(self.tree),
                        );
                        child.insert_value(
                            value,
                            pos!(0),
                            I::Int::B / 2 + 1,
                            child_pool(self.tree),
                        );

                        // Stealing the last element of `sibling` has caused
                        // its largest pivot to decrease. Update the pivot for this
                        // sub-tree in the parent to the pivot for the new last
                        // element.
                        let sibling_max_pivot =
                            sibling.pivot(sibling_last.prev(), child_pool(self.tree));
                        node.set_pivot(sibling_max_pivot, pos.prev(), &mut self.tree.internal);

                        // Now actually shrink the sibling by removing its last
                        // element.
                        sibling.set_pivot(I::Int::MAX, sibling_last, child_pool(self.tree));
                    } else {
                        // If the child is a internal node then we need to
                        // recover the maximum pivot in the sibling from `node`
                        // and insert that along with the last sub-tree in the
                        // sibling into the child.
                        //
                        // We use a node size of `B / 2 + 1` so that the
                        // operation becomes a copy of exactly `B / 2` elements.
                        // All elements in the second half of the node are
                        // absent anyways. This also preserves the next leaf
                        // pointer.
                        let sibling_max_pivot = node.pivot(pos.prev(), &self.tree.internal);
                        let value = sibling
                            .value(sibling_last, child_pool(self.tree))
                            .assume_init_read();
                        child.insert_pivot(
                            sibling_max_pivot,
                            pos!(0),
                            I::Int::B / 2 + 1,
                            child_pool(self.tree),
                        );
                        child.insert_value(
                            value,
                            pos!(0),
                            I::Int::B / 2 + 1,
                            child_pool(self.tree),
                        );

                        // Stealing the last element of `sibling` has caused
                        // its largest pivot to decrease. Update the pivot for this
                        // sub-tree in the parent to the pivot for the new last
                        // element.
                        let sibling_max_pivot =
                            sibling.pivot(sibling_last.prev(), child_pool(self.tree));
                        node.set_pivot(sibling_max_pivot, pos.prev(), &mut self.tree.internal);

                        // Now actually shrink the sibling by removing its last
                        // element.
                        sibling.set_pivot(I::Int::MAX, sibling_last.prev(), child_pool(self.tree));
                    }

                    // After stealing, we need to adjust the cursor position for
                    // the child.
                    self.stack[height].1 = self.stack[height].1.next();
                }

                // Stealing can't cause recursive underflows.
                None
            } else {
                unsafe {
                    // The child has exactly `B / 2 - 1` elements, move those to
                    // the end of the sibling which has exactly `B / 2`
                    // elements. This results in a full node with the maximum of
                    // `B - 1` elements.
                    sibling.merge_from(
                        child,
                        pos!(I::Int::B / 2),
                        I::Int::B / 2 - 1,
                        child_pool(self.tree),
                    );

                    // If this is an internal node then we need to copy the
                    // previous maximum pivot for the sibling's sub-tree to slot
                    // `B / 2 - 1`  which previously contained MAX.
                    if !child_is_leaf {
                        let sibling_max_pivot = node.pivot(pos.prev(), &self.tree.internal);
                        sibling.set_pivot(
                            sibling_max_pivot,
                            pos!(I::Int::B / 2 - 1),
                            child_pool(self.tree),
                        );
                    }

                    // Update the next leaf pointer if this is a leaf node.
                    if child_is_leaf {
                        let next_leaf = child.next_leaf(child_pool(self.tree));
                        sibling.set_next_leaf(next_leaf, child_pool(self.tree));
                    }

                    // The child is no longer in the tree, free its node.
                    child_pool(self.tree).free_node(child);

                    // Remove the child node from its parent. We keep the pivot
                    // of `child` and remove that of `sibling` because the pivot
                    // should hold the maximum pivot in the sub-tree.
                    node.remove_pivot(pos.prev(), &mut self.tree.internal);
                    node.remove_value(pos, &mut self.tree.internal);

                    // After merging, we need to adjust the cursor position for
                    // the child and parent.
                    self.stack[up].1 = self.stack[up].1.prev();
                    self.stack[height] = (
                        sibling,
                        NodePos::new_unchecked(self.stack[height].1.index() + I::Int::B / 2),
                    );
                }

                // Merging may cause the parent node to become under-sized.
                Some(node)
            }
        }
    }
}

/// A cursor over the elements of a [`RangeTree`].
///
/// Cursors point either to an element in the tree or to the end of the tree.
///
/// Iterators are more efficient than cursors. Prefer using them if you don't
/// need reverse iteration or if you don't need to insert or remove elements in
/// the tree.
///
/// This type is returned by [`RangeTree::cursor_at`] and [`RangeTree::cursor`].
pub struct Cursor<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    raw: RawCursor<I, V, A, &'a RangeTree<I, V, A>>,
}

impl<I: RangeTreeIndex, V, A: Allocator> Clone for Cursor<'_, I, V, A> {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            raw: self.raw.clone(),
        }
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Cursor<'a, I, V, A> {
    /// Returns `true` if the cursor points to the end of the tree.
    #[inline]
    pub fn is_end(&self) -> bool {
        self.raw.is_end()
    }

    /// Returns the pivot of the element that the cursor is currently pointing to,
    /// or `None` if the cursor is pointing to the end of the tree.
    #[inline]
    pub fn range(&self) -> Option<ops::Range<I>> {
        self.entry().map(|(r, _v)| r)
    }

    /// Returns a reference to the value that the cursor is currently
    /// pointing to, or `None` if the cursor is pointing to the end of the tree.
    #[inline]
    pub fn value(&self) -> Option<&'a V> {
        self.entry().map(|(_k, v)| v)
    }

    /// Returns the pivot and a reference to the value that the cursor is
    /// currently pointing to, or `None` if the cursor is pointing to the end of
    /// the tree.
    #[inline]
    pub fn entry(&self) -> Option<(ops::Range<I>, &'a V)> {
        self.raw.entry().map(|(pivot, value)| {
            let (start, value) = unsafe { value.as_ref() };

            (*start..pivot, value)
        })
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

    /// Returns an iterator starting a the current element.
    ///
    /// Iterators are more efficient than cursors. Prefer using them if you don't
    /// need reverse iteration or if you don't need to insert or remove elements in
    /// the tree.
    #[inline]
    pub fn iter(&self) -> Iter<'a, I, V, A> {
        let (node, pos) = self.raw.stack[Height::LEAF];
        Iter {
            raw: crate::RawIter { node, pos },
            tree: self.raw.tree,
        }
    }
}

/// A mutable cursor over the elements of a [`RangeTree`] which allows editing
/// operations.
///
/// Cursors point either to an element in the tree or to the end of the tree.
///
/// Iterators are more efficient than cursors. Prefer using them if you don't
/// need reverse iteration or if you don't need to insert or remove elements in
/// the tree.
///
/// This type is returned by [`RangeTree::cursor_mut_at`] and [`RangeTree::cursor_mut`].
pub struct CursorMut<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    raw: RawCursor<I, V, A, &'a mut RangeTree<I, V, A>>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> CursorMut<'a, I, V, A> {
    /// Internal constructor for an uninitialized cursor.
    ///
    /// This allows cursors to be initialized in-place, which works around
    /// rustc's poor support for move-elimination.
    ///
    /// # Safety
    ///
    /// The cursor must be initialized before use by calling `seek`.
    #[inline]
    pub(crate) unsafe fn uninit(tree: &'a mut RangeTree<I, V, A>) -> Self {
        Self {
            raw: RawCursor {
                stack: <I::Int as RangeTreeInteger>::Stack::default(),
                tree,
            },
        }
    }

    /// Initializes a cursor to point to the given pivot.
    #[inline]
    pub(crate) fn seek(&mut self, pivot: <I::Int as RangeTreeInteger>::Raw) {
        self.raw.seek(pivot);
    }

    /// Returns `true` if the cursor points to the end of the tree.
    #[inline]
    pub fn is_end(&self) -> bool {
        self.entry().is_none()
    }

    /// Returns the pivot of the element that the cursor is currently pointing to,
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

    /// Returns the pivot and a reference to the value that the cursor is
    /// currently pointing to, or `None` if the cursor is pointing to the end of
    /// the tree.
    #[inline]
    pub fn entry(&self) -> Option<(ops::Range<I>, &V)> {
        self.raw.entry().map(|(pivot, value)| {
            let (start, value) = unsafe { value.as_ref() };

            (*start..pivot, value)
        })
    }

    /// Returns the pivot and a mutable reference to the value that the cursor is
    /// currently pointing to, or `None` if the cursor is pointing to the end of
    /// the tree.
    #[inline]
    pub fn entry_mut(&mut self) -> Option<(ops::Range<I>, &mut V)> {
        self.raw.entry().map(|(pivot, mut value)| {
            let (start, value) = unsafe { value.as_mut() };

            (*start..pivot, value)
        })
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

    /// Returns an iterator starting a the current element.
    ///
    /// Iterators are more efficient than cursors. Prefer using them if you don't
    /// need reverse iteration or if you don't need to insert or remove elements in
    /// the tree.
    #[inline]
    pub fn iter(&self) -> Iter<'_, I, V, A> {
        let (node, pos) = self.raw.stack[Height::LEAF];
        Iter {
            raw: crate::RawIter { node, pos },
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
            raw: crate::RawIter { node, pos },
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
            raw: crate::RawIter { node, pos },
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
            raw: crate::RawIter { node, pos },
            tree: self.raw.tree,
        }
    }

    /// Inserts `pivot` and `value` before the element that the cursor is
    /// currently pointing to.
    ///
    /// After insertion the cursor will be pointing to the newly inserted
    /// element.
    ///
    /// If the cursor is pointing to the end of the tree then this inserts the
    /// new element at the end of the tree after all other elements.
    ///
    /// It is the user's responsibility to ensure that inserting `pivot` at this
    /// position does not violate the invariant that all pivots must be in sorted
    /// order in the tree. Violating this invariant is safe but may cause
    /// other operations to return incorrect results or panic.
    #[inline]
    pub fn insert_before(&mut self, range: ops::Range<I>, value: V) -> Result<(), AllocError> {
        self.raw.insert::<false>(range, value)
    }

    /// Inserts `pivot` and `value` after the element that the cursor is
    /// currently pointing to.
    ///
    /// After insertion the cursor will still be pointing to the same element as
    /// before the insertion.
    ///
    /// It is the user's responsibility to ensure that inserting `pivot` at this
    /// position does not violate the invariant that all pivots must be in sorted
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

    /// Replaces the pivot and value of the element that the cursor is currently
    /// pointing to and returns the previous pivot and value.
    ///
    /// It is the user's responsibility to ensure that inserting `pivot` at this
    /// position does not violate the invariant that all pivots must be in sorted
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

    /// Removes the element that the cursor is currently pointing to and returns
    /// it.
    ///
    /// After removal the cursor will point to the element after the current
    /// one.
    ///
    /// # Panics
    ///
    /// Panics if the cursor is pointing to the end of the tree.
    #[inline]
    pub fn remove(&mut self) -> (ops::Range<I>, V) {
        self.raw.remove()
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
            node = unsafe { node.value(pos!(0), &tree.internal).assume_init_read().0 };
            height = down;
        }

        // The first leaf node is always the left-most leaf on the tree and is
        // never deleted.
        debug_assert_eq!(node, NodeRef::ZERO);
        stack[height] = (NodeRef::ZERO, pos!(0));
        RawCursor { stack, tree }
    }

    /// Returns a [`RawCursor`] pointing at the first element with pivot greater
    /// than `bound`.
    #[inline]
    fn raw_cursor_at<Ref: Deref<Target = Self>>(
        tree: Ref,
        pivot: <I::Int as RangeTreeInteger>::Raw,
    ) -> RawCursor<I, V, A, Ref> {
        let stack = <I::Int as RangeTreeInteger>::Stack::default();
        let mut cursor = RawCursor { stack, tree };
        cursor.seek(pivot);
        cursor
    }

    /// Returns a [`Cursor`] pointing at the first element of the tree.
    #[inline]
    pub fn cursor(&self) -> Cursor<'_, I, V, A> {
        let raw = Self::raw_cursor(self);
        Cursor { raw }
    }

    /// Returns a [`Cursor`] pointing at the first element with pivot greater
    /// than `bound`.
    #[inline]
    pub fn cursor_at(&self, bound: Bound<I>) -> Cursor<'_, I, V, A> {
        let pivot = match bound {
            Bound::Included(pivot) => int_from_pivot(pivot),
            Bound::Excluded(pivot) => I::Int::increment(int_from_pivot(pivot)),
            Bound::Unbounded => I::Int::MAX,
        };
        let raw = Self::raw_cursor_at(self, pivot);
        Cursor { raw }
    }

    /// Returns a [`CursorMut`] pointing at the first element of the tree.
    #[inline]
    pub fn cursor_mut(&mut self) -> CursorMut<'_, I, V, A> {
        let raw = Self::raw_cursor(self);
        CursorMut { raw }
    }

    /// Returns a [`CursorMut`] pointing at the first element with pivot greater
    /// than `bound`.
    #[inline]
    pub fn cursor_mut_at(&mut self, bound: Bound<I>) -> CursorMut<'_, I, V, A> {
        let pivot = match bound {
            Bound::Included(pivot) => int_from_pivot(pivot),
            Bound::Excluded(pivot) => I::Int::increment(int_from_pivot(pivot)),
            Bound::Unbounded => I::Int::MAX,
        };
        let raw = Self::raw_cursor_at(self, pivot);
        CursorMut { raw }
    }
}
