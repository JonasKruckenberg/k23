use core::alloc::{AllocError, Allocator};
use core::ops;
use core::ops::Deref;

use crate::RangeTree;
use crate::idx::Idx;
use crate::node::{marker, pos};
use crate::stack::Height;

struct RawCursor<I: Idx, V, A: Allocator, Ref: Deref<Target = RangeTree<I, V, A>>> {
    tree: Ref,
    stack: I::Stack<V>,
}

impl<I: Idx, V, A: Allocator, Ref: Deref<Target = RangeTree<I, V, A>>> RawCursor<I, V, A, Ref> {
    #[inline]
    fn seek(&mut self, search: I::Raw) {
        // Go down the tree, at each internal node selecting the first sub-tree
        // with key greater than or equal to the search key. This sub-tree will
        // only contain keys less than or equal to its key.
        let mut height = self.tree.height;
        let mut node = self.tree.root;
        while let Some(down) = height.down() {
            let n = unsafe { node.cast::<marker::Internal<V>>() };
            let pivots = unsafe { n.pivots(&self.tree.internal) };
            let pos = unsafe { I::search(pivots, search) };
            self.stack[height] = (node, pos);
            node = unsafe { n.child(pos, &self.tree.internal).assume_init_read() };
            height = down;
        }

        // Select the first leaf element with key greater than or equal to the
        // search.
        let n = unsafe { node.cast::<marker::Leaf<V>>() };
        let keys = unsafe { n.pivots(&self.tree.leaf) };
        let pos = unsafe { I::search(keys, search) };
        self.stack[height] = (node, pos);
    }

    /// Returns `true` if the cursor points to the end of the tree.
    #[inline]
    fn is_end(&self) -> bool {
        let (node, pos) = self.stack[Height::LEAF];
        let key = unsafe { node.cast().pivot(pos, &self.tree.leaf) };
        key == I::MAX
    }

    fn assert_valid(&self) {
        // The element at each internal level should point to the node lower on
        // the stack.
        let mut height = Height::LEAF;
        while let Some(up) = height.up(self.tree.height) {
            let (node, pos) = self.stack[up];
            let child = self.stack[height].0;

            debug_assert_eq!(
                unsafe { node.cast().child(pos, &self.tree.internal).assume_init_read() },
                child
            );

            height = up;
        }

        // If the leaf node points to an `Int::MAX` key then so must all
        // internal nodes.
        let (node, pos) = self.stack[Height::LEAF];
        if unsafe { node.cast().pivot(pos, &self.tree.leaf) } == I::MAX {
            let mut height = Height::LEAF;
            while let Some(up) = height.up(self.tree.height) {
                let (node, pos) = self.stack[up];
                assert_eq!(
                    unsafe { node.cast().pivot(pos, &self.tree.internal) },
                    I::MAX
                );
                height = up;
            }
        }

        assert_eq!(self.stack[self.tree.height].0, self.tree.root);
    }
}

impl<I: Idx, V, A: Allocator> RawCursor<I, V, A, &'_ mut RangeTree<I, V, A>> {
    #[inline]
    unsafe fn update_leaf_max_key(&mut self, key: I::Raw) {
        let mut height = Height::LEAF;
        // This continues recursively as long as the parent sub-tree is the last
        // one in its node, or the root of the tree is reached.
        while let Some(up) = height.up(self.tree.height) {
            let (node, pos) = self.stack[up];
            let node = unsafe { node.cast() };

            if unsafe { node.pivot(pos, &self.tree.internal) } != I::MAX {
                unsafe {
                    node.set_pivot(key, pos, &mut self.tree.internal);
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
        let (node, pos) = self.stack[Height::LEAF];
        let node = unsafe { node.cast::<marker::Leaf<V>>() };

        let insert_pos = if AFTER {
            assert!(
                !self.is_end(),
                "called insert_after() on cursor already at end"
            );
            unsafe { pos.next() }
        } else {
            pos
        };
        let prev_key = unsafe { node.pivot(insert_pos, &self.tree.leaf) };

        // If we are inserting the last key in a node then we need to update
        // the sub-tree max key in the parent.
        if prev_key == I::MAX {
            if AFTER {
                unsafe {
                    self.update_leaf_max_key(range.end.to_raw());
                }
            } else {
                // Note that because of the cursor invariants we don't need to
                // update the sub-tree keys in any parent nodes:
                // - If the cursor is at the end of the tree then all keys on
                //   the stack have value `Int::MAX` already.
                // - Otherwise the insertion doesn't happen at the end of the
                //   node, so the maximum key doesn't change.
                debug_assert!(self.is_end());
            }
        }

        // Check if this insertion will cause the leaf node to become completely
        // full. Specifically that after insertion the last key will *not* be
        // `Int::MAX`, which violates the node invariant.
        let overflow = unsafe { node.pivot(pos!(I::B - 2), &self.tree.leaf) } != I::MAX;

        // Save the next leaf pointer since it is overwritten by insertion.
        let next_leaf = unsafe { node.next_leaf(&self.tree.leaf) };

        // Insert the new key and value in the leaf. Use a fast path for
        // inserting at the end of a node. This helps with common cases when
        // appending to the end of a tree.
        if prev_key == I::MAX {
            unsafe {
                node.set_pivot(range.end.to_raw(), insert_pos, &mut self.tree.leaf);
                node.start_mut(insert_pos, &mut self.tree.leaf).write(range.start.to_raw());
                node.value_mut(insert_pos, &mut self.tree.leaf).write(value);
            }
        } else {
            unsafe {
                node.insert_pivot(range.end.to_raw(), insert_pos, I::B, &mut self.tree.leaf);
                node.insert_start(range.start.to_raw(), insert_pos, I::B, &mut self.tree.leaf);
                node.insert_value(value, insert_pos, I::B, &mut self.tree.leaf);
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

        // At this point the leaf node is completely full and needs to be split
        // to maintain the node invariant.

        // Record the last key of the first half of the node. This will become
        // the key for the left sub-tree in the parent node.
        let mut mid_key = unsafe { node.pivot(pos!(I::B / 2 - 1), &self.tree.leaf) };

        // Allocate a new node and move the second half of the current node to
        // it.
        let new_uninit_node = unsafe { self.tree.leaf.alloc_node(&self.tree.allocator)? };
        let new_node = unsafe { node.split_into(new_uninit_node, &mut self.tree.leaf) };

        // Update the next-leaf pointers for both nodes.
        unsafe {
            new_node.set_next_leaf(next_leaf, &mut self.tree.leaf);
            node.set_next_leaf(Some(new_node), &mut self.tree.leaf);
        }

        let mut new_node = unsafe { new_node.cast() };

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
            let node = unsafe { node.cast() };

            // The last 2 keys of leaf nodes are always `Int::MAX` so we can
            // check if an insertion will cause an overflow by looking at
            // whether the key at `B - 3` is `Int::MAX`.
            let overflow = unsafe { node.pivot(pos!(I::B - 3), &self.tree.internal) } != I::MAX;

            // The existing key for this sub-tree (max of all keys in sub-tree)
            // is correct for the second node of the split. Similarly the
            // existing value already points to the first node of the split. So
            // insert the new key before the existing one and the new value
            // after the existing one.
            unsafe {
                node.insert_pivot(mid_key, pos, I::B, &mut self.tree.internal);
                node.insert_child(new_node, pos.next(), I::B, &mut self.tree.internal);
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

            // Record the last key of the first half of the node. This will
            // become the key for the left sub-tree in the parent node.
            mid_key = unsafe { node.pivot(pos!(I::B / 2 - 1), &self.tree.internal) };

            // Set the last key of the first half to `Int::MAX` to indicate that
            // it is the last element in this node.
            unsafe {
                node.set_pivot(I::MAX, pos!(I::B / 2 - 1), &mut self.tree.internal);
            }

            // Allocate a new node and move the second half of the current node
            // to it.
            let new_uninit_node = unsafe { self.tree.internal.alloc_node(&self.tree.allocator)? };
            new_node = unsafe {
                node.split_into(new_uninit_node, &mut self.tree.internal)
                    .cast()
            };

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
        let new_uninit_root = unsafe { self.tree.internal.alloc_node(&self.tree.allocator)? };

        // The new root only contains 2 elements: the original root node and the
        // newly created split node. The only non-MAX key is the first one which
        // holds the maximum key in the left sub-tree.
        let new_root = unsafe {
            let new_root = new_uninit_root.init_pivots(&mut self.tree.internal);
            new_root.set_pivot(mid_key, pos!(0), &mut self.tree.internal);
            new_root
                .child_mut(pos!(0), &mut self.tree.internal)
                .write(self.tree.root);
            new_root
                .child_mut(pos!(1), &mut self.tree.internal)
                .write(new_node);

            new_root.cast()
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
}

pub struct Cursor<'a, I: Idx, V, A: Allocator> {
    raw: RawCursor<I, V, A, &'a RangeTree<I, V, A>>,
}

pub struct CursorMut<'a, I: Idx, V, A: Allocator> {
    raw: RawCursor<I, V, A, &'a mut RangeTree<I, V, A>>,
}

impl<'a, I: Idx, V, A: Allocator> CursorMut<'a, I, V, A> {
    #[inline]
    pub(crate) unsafe fn uninit(tree: &'a mut RangeTree<I, V, A>) -> Self {
        Self {
            raw: RawCursor {
                tree,
                stack: I::Stack::default(),
            },
        }
    }

    #[inline]
    pub(crate) fn seek(&mut self, search: I::Raw) {
        self.raw.seek(search);
    }

    #[inline]
    pub fn insert_before(&mut self, range: ops::Range<I>, value: V) -> Result<(), AllocError> {
        self.raw.insert::<false>(range, value)
    }

    #[inline]
    pub fn insert_after(&mut self, range: ops::Range<I>, value: V) -> Result<(), AllocError> {
        self.raw.insert::<true>(range, value)
    }
}
