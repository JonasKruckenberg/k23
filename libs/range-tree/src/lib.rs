#![cfg_attr(not(test), no_std)]
#![feature(allocator_api)]
extern crate alloc;

mod cursor;
mod int;
mod iter;
mod node;
mod simd;
mod stack;

use alloc::alloc::Global;
use core::alloc::{AllocError, Allocator};
use core::fmt::{self, Debug, Display};
use core::{mem, ops};
use core::ops::Bound;
pub use cursor::{Cursor, CursorMut};
use int::RangeTreeInteger;
pub use iter::{IntoIter, Iter, IterMut, Ranges, Values, ValuesMut};
pub use nonmax;

use crate::node::{NodePool, NodePos, NodeRef, UninitNodeRef, marker, pos};
use crate::stack::Height;

/// Error type returned by insertion methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertError {
    /// An allocation failure occurred while inserting.
    AllocError,
    /// The range overlaps with an existing range in the tree.
    Overlap,
}

impl Display for InsertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InsertError::AllocError => write!(f, "allocation failure"),
            InsertError::Overlap => write!(f, "overlapping range"),
        }
    }
}

impl From<AllocError> for InsertError {
    fn from(_: AllocError) -> Self {
        InsertError::AllocError
    }
}

pub trait RangeTreeIndex: Copy {
    #[allow(private_bounds)]
    type Int: RangeTreeInteger;

    /// Converts the index to an integer.
    fn to_int(self) -> Self::Int;

    /// Recovers the index from an integer.
    fn from_int(int: Self::Int) -> Self;
}

pub struct RangeTree<I: RangeTreeIndex, V, A: Allocator = Global> {
    internal: NodePool<I::Int, marker::Internal>,
    leaf: NodePool<I::Int, marker::Leaf<V>>,
    root: NodeRef,
    height: Height<I::Int>,
    allocator: A,
}

impl<I: RangeTreeIndex, V, A: Allocator> RangeTree<I, V, A> {
    #[inline]
    pub fn try_new_in(allocator: A) -> Result<Self, AllocError> {
        let mut out = Self {
            internal: NodePool::new(),
            leaf: NodePool::new(),
            height: Height::LEAF,
            root: NodeRef::ZERO,
            allocator,
        };
        let root = unsafe { out.leaf.alloc_node(&out.allocator)? };
        out.init_root(root);
        Ok(out)
    }

    /// Initializes the root node to the leaf node at offset zero.
    #[inline]
    fn init_root(&mut self, root: UninitNodeRef) {
        let root = unsafe { root.init_pivots(&mut self.leaf) };
        unsafe {
            root.set_next_leaf(None, &mut self.leaf);
        }
        debug_assert_eq!(root, NodeRef::ZERO);
        self.root = NodeRef::ZERO;
    }

    /// Clears the map, removing all elements.
    #[inline]
    pub fn clear(&mut self) {
        // Drop values. We don't need to modify the keys since we're about to
        // free the nodes anyways.
        if mem::needs_drop::<V>() {
            let mut iter = self.raw_iter();
            while let Some((_key, value_ptr)) = unsafe { iter.next(&self.leaf) } {
                unsafe {
                    value_ptr.drop_in_place();
                }
            }
        }

        // Free all nodes without freeing the underlying allocations.
        let root = self.leaf.clear_and_alloc_node();
        self.internal.clear();

        // Re-initialize the root node.
        self.height = Height::LEAF;
        self.init_root(root);
    }

    /// Returns `true` if the map contains no elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        if self.height != Height::LEAF {
            return false;
        }
        let first_key = unsafe { self.root.pivot(pos!(0), &self.leaf) };
        first_key == I::Int::MAX
    }

    #[inline]
    pub fn insert(&mut self, range: ops::Range<I>, value: V) -> Result<(), InsertError> {
        let mut cursor = unsafe { CursorMut::uninit(self) };
        cursor.seek(range.end.to_int().to_raw());

        if let Some((existing, _)) = cursor.entry()
            && I::Int::cmp(
                existing.start.to_int().to_raw(),
                range.end.to_int().to_raw(),
            )
            .is_lt()
        {
            return Err(InsertError::Overlap);
        }

        if cursor.prev() {
            if let Some((prev, _)) = cursor.entry()
                && I::Int::cmp(prev.end.to_int().to_raw(), range.start.to_int().to_raw()).is_gt()
            {
                // Overlap detected: previous range ends after new range starts
                return Err(InsertError::Overlap);
            }

            cursor.next(); // Move back to insertion position
        }

        cursor.insert_before(range, value)?;
        Ok(())
    }

    #[inline]
    pub fn get(&self, search: I) -> Option<&V> {
        let cursor = self.cursor_at(Bound::Included(search));
        cursor.into_iter().next().map(|(_range, value)| value)
    }

    pub fn assert_valid(&self, assert_sorted: bool) {
        let mut last_leaf = None;
        self.check_node(
            self.root,
            self.height,
            assert_sorted,
            None,
            I::Int::MAX,
            &mut last_leaf,
        );

        // Ensure the linked list of leaf nodes is properly terminated.
        assert_eq!(unsafe { last_leaf.unwrap().next_leaf(&self.leaf) }, None);
    }

    fn check_node(
        &self,
        node: NodeRef,
        height: Height<I::Int>,
        assert_sorted: bool,
        min: Option<<I::Int as RangeTreeInteger>::Raw>,
        max: <I::Int as RangeTreeInteger>::Raw,
        prev_leaf: &mut Option<NodeRef>,
    ) {
        let Some(down) = height.down() else {
            self.check_leaf_node(node, assert_sorted, min, max, prev_leaf);
            return;
        };

        let keys = || {
            (0..I::Int::B).map(|i| unsafe { node.pivot(NodePos::new_unchecked(i), &self.internal) })
        };

        // The last 2 keys must be MAX.
        assert_eq!(keys().nth(I::Int::B - 1).unwrap(), I::Int::MAX);
        assert_eq!(keys().nth(I::Int::B - 2).unwrap(), I::Int::MAX);

        // All MAX keys must be after non-MAX keys,
        assert!(keys().is_sorted_by_key(|key| key == I::Int::MAX));

        // Keys must be sorted in increasing order.
        if assert_sorted {
            assert!(keys().is_sorted_by(|&a, &b| I::Int::cmp(a, b).is_le()));
            if let Some(min) = min {
                assert!(keys().all(|key| I::Int::cmp(key, min).is_ge()));
            }
            assert!(keys().all(|key| key == I::Int::MAX || I::Int::cmp(key, max).is_le()));
        }

        let len = keys().take_while(|&key| key != I::Int::MAX).count() + 1;
        let is_root = height == self.height;

        // Non-root nodes must be at least half full. Non-leaf root nodes must
        // have at least 2 elements.
        if is_root {
            assert!(len >= 2);
        } else {
            assert!(len >= I::Int::B / 2);
        }

        // Check the invariants for child nodes.
        let mut prev_key = min;
        for i in 0..len {
            unsafe {
                let pos = NodePos::new_unchecked(i);
                let key = node.pivot(pos, &self.internal);
                let child = node.child(pos, &self.internal).assume_init_read();
                self.check_node(
                    child,
                    down,
                    assert_sorted,
                    prev_key,
                    if key == I::Int::MAX { max } else { key },
                    prev_leaf,
                );
                prev_key = Some(key);
            }
        }
    }

    fn check_leaf_node(
        &self,
        node: NodeRef,
        assert_sorted: bool,
        min: Option<<I::Int as RangeTreeInteger>::Raw>,
        max: <I::Int as RangeTreeInteger>::Raw,
        prev_leaf: &mut Option<NodeRef>,
    ) {
        let keys =
            || (0..I::Int::B).map(|i| unsafe { node.pivot(NodePos::new_unchecked(i), &self.leaf) });

        // The last key must be MAX.
        assert_eq!(keys().nth(I::Int::B - 1).unwrap(), I::Int::MAX);

        // All MAX keys must be after non-MAX keys,
        assert!(keys().is_sorted_by_key(|key| key == I::Int::MAX));

        // Keys must be sorted in increasing order.
        if assert_sorted {
            assert!(keys().is_sorted_by(|&a, &b| I::Int::cmp(a, b).is_le()));
            if let Some(min) = min {
                assert!(keys().all(|key| I::Int::cmp(key, min).is_ge()));
            }
            assert!(keys().all(|key| key == I::Int::MAX || I::Int::cmp(key, max).is_le()));
        }

        let len = keys().take_while(|&key| key != I::Int::MAX).count();
        let is_root = self.height == Height::LEAF;

        // Non-root nodes must be at least half full.
        if !is_root {
            assert!(len >= I::Int::B / 2);
        }

        // The last key must be equal to the maximum for this sub-tree.
        if max != I::Int::MAX {
            assert_eq!(keys().nth(len - 1).unwrap(), max);
        }

        // The first leaf node must always have an offset of 0.
        if prev_leaf.is_none() {
            assert_eq!(node, NodeRef::ZERO);
        }

        // Ensure the linked list of leaf nodes is correct.
        if let Some(prev_leaf) = prev_leaf {
            assert_eq!(unsafe { prev_leaf.next_leaf(&self.leaf) }, Some(node));
        }

        *prev_leaf = Some(node);
    }
}

impl<I: RangeTreeIndex, V, A: Allocator> Drop for RangeTree<I, V, A> {
    #[inline]
    fn drop(&mut self) {
        // Drop values. We don't need to modify the keys since we're about to
        // free the nodes anyways.
        if mem::needs_drop::<V>() {
            let mut iter = self.raw_iter();
            while let Some((_key, value_ptr)) = unsafe { iter.next(&self.leaf) } {
                unsafe {
                    value_ptr.drop_in_place();
                }
            }
        }

        // Release all allocated memory
        unsafe {
            self.internal.clear_and_free(&self.allocator);
            self.leaf.clear_and_free(&self.allocator);
        }
    }
}