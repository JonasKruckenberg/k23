//! This crate provides [`RangeTree`], a fast B+ Tree implementation using integer
//! pivots.

#![cfg_attr(not(test), no_std)]
#![feature(allocator_api)]
#![warn(missing_docs)]

extern crate alloc;

use alloc::alloc::{Allocator, Global};
use core::alloc::AllocError;
use core::ops::Bound;
use core::{fmt, mem, ops};

use int::RangeTreeInteger;
use node::{NodePool, NodeRef, UninitNodeRef};
use stack::Height;

#[macro_use]
mod node;

mod cursor;
mod int;
mod iter;
mod simd;
mod stack;
// #[cfg(test)]
// mod tests;

pub use cursor::*;
pub use iter::*;

use crate::int::int_from_pivot;
use crate::node::NodePos;

/// Error type returned by insertion methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertError {
    /// An allocation failure occurred while inserting.
    AllocError,
    /// The range overlaps with an existing range in the tree.
    Overlap,
}

impl fmt::Display for InsertError {
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

/// Trait which must be implemented for all pivots inserted into a [`RangeTree`].
///
/// [`RangeTree`] requires that pivots be integers and reserves the maximum integer
/// value for internal use. This trait is already implementated for all integers
/// from the [`nonmax`] crate, but this crate allows for custom pivot types that
/// are convertible to/from an integer.
///
/// Note that pivots in the [`RangeTree`] are ordered by their integer value and not
/// the [`Ord`] implementation of the pivot type.
pub trait RangeTreeIndex: Copy {
    /// Non-max integer type that this pivot
    ///
    /// This must be one of the integer types from the [`nonmax`] crate:
    /// - [`nonmax::NonZeroU8`]
    /// - [`nonmax::NonZeroU16`]
    /// - [`nonmax::NonZeroU32`]
    /// - [`nonmax::NonZeroU64`]
    /// - [`nonmax::NonZeroU128`]
    /// - [`nonmax::NonZeroI8`]
    /// - [`nonmax::NonZeroI16`]
    /// - [`nonmax::NonZeroI32`]
    /// - [`nonmax::NonZeroI64`]
    /// - [`nonmax::NonZeroI128`]
    #[allow(private_bounds)]
    type Int: RangeTreeInteger;

    // const ZERO: Self;
    // const MAX: Self;

    /// Converts the pivot to an integer.
    fn to_int(self) -> Self::Int;

    /// Recovers the pivot from an integer.
    fn from_int(int: Self::Int) -> Self;
}

/// An ordered map based on a [B+ Tree].
///
/// This is similar to the standard library's `BTreeMap` but differs in several
/// ways:
/// - Lookups and insertions are 2-4x faster than `BTreeMap`.
/// - `BTree` can optionally be used as a multi-map and hold duplicate pivots.
/// - pivots must be `Copy` and convertible to and from integers via the
///   [`RangeTreeIndex`] trait.
/// - The maximum integer value is reserved for internal use and cannot be used
///   by pivots.
/// - Elements in the tree are ordered by the integer value of the pivot instead
///   of the [`Ord`] implementation of the pivots.
/// - [`Cursor`] and [`CursorMut`] can be used to seek back-and-forth in the
///   tree while inserting or removing elements.
/// - Iterators only support forward iteration.
///
/// The data structure design is based on the [B- Tree] by Sergey Slotin, but
/// has been significantly extended.
///
/// [B+ Tree]: https://en.wikipedia.org/wiki/B%2B_tree
/// [B- Tree]: https://en.algorithmica.org/hpc/data-structures/b-tree/
pub struct RangeTree<I: RangeTreeIndex, V, A: Allocator = Global> {
    internal: NodePool<I::Int, (NodeRef, u32)>,
    leaf: NodePool<I::Int, (I, V)>,
    height: Height<I::Int>,
    root: NodeRef,
    alloc: A,
}

impl<I: RangeTreeIndex + fmt::Debug, V: fmt::Debug, A: Allocator> fmt::Debug
    for RangeTree<I, V, A>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<I: RangeTreeIndex, V> RangeTree<I, V, Global> {
    /// Creates a new, empty [`RangeTree`].
    ///
    /// This requires an initial memory allocation on creation.
    #[inline]
    pub fn try_new() -> Result<Self, AllocError> {
        Self::try_new_in(Global)
    }
}

impl<I: RangeTreeIndex, V, A: Allocator> RangeTree<I, V, A> {
    /// Creates a new, empty [`RangeTree`] with the given allocator.
    ///
    /// This requires an initial memory allocation on creation.
    #[inline]
    pub fn try_new_in(alloc: A) -> Result<Self, AllocError> {
        let mut out = Self {
            internal: NodePool::new(),
            leaf: NodePool::new(),
            height: Height::LEAF,
            root: NodeRef::ZERO,
            alloc,
        };
        let root = unsafe { out.leaf.alloc_node(&out.alloc)? };
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
        // Drop values. We don't need to modify the pivots since we're about to
        // free the nodes anyways.
        if mem::needs_drop::<V>() {
            let mut iter = self.raw_iter();
            while let Some((_pivot, value_ptr)) = unsafe { iter.next(&self.leaf) } {
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
        let first_pivot = unsafe { self.root.pivot(pos!(0), &self.leaf) };
        first_pivot == I::Int::MAX
    }

    /// Returns a reference to the value corresponding to the pivot.
    #[inline]
    pub fn get(&self, search: I) -> Option<&V> {
        let cursor = self.cursor_at(Bound::Included(search));
        let (range, value) = cursor.iter().next()?;
        if range.start.to_int().to_raw() <= search.to_int().to_raw() {
            Some(value)
        } else {
            None
        }
    }

    /// Returns a mutable reference to the value corresponding to the pivot.
    #[inline]
    pub fn get_mut(&mut self, search: I) -> Option<&mut V> {
        let cursor = self.cursor_mut_at(Bound::Included(search));
        let (range, value) = cursor.into_iter_mut().next()?;
        if range.start.to_int().to_raw() <= search.to_int().to_raw() {
            Some(value)
        } else {
            None
        }
    }

    /// Inserts a pivot-value pair into the map while allowing for multiple
    /// identical pivots.
    ///
    /// This allows the `BTree` to be used as a multi-map where each pivot can
    /// have multiple values. In this case [`RangeTree::get`], [`RangeTree::get_mut`]
    /// and [`RangeTree::remove`] will only operate on one of the associated values
    /// (arbitrarily chosen).
    #[inline]
    pub fn insert(&mut self, range: ops::Range<I>, value: V) -> Result<(), InsertError> {
        let mut cursor = unsafe { CursorMut::uninit(self) };
        cursor.seek(int_from_pivot(range.end));

        if let Some((existing, _)) = cursor.entry()
            && existing.start.to_int().to_raw() < range.end.to_int().to_raw()
        {
            return Err(InsertError::Overlap);
        }

        if cursor.prev() {
            if let Some((prev, _)) = cursor.entry()
                && prev.end.to_int().to_raw() > range.start.to_int().to_raw()
            {
                // Overlap detected: previous range ends after new range starts
                return Err(InsertError::Overlap);
            }

            cursor.next(); // Move back to insertion position
        }

        cursor.insert_before(range, value)?;

        Ok(())
    }

    /// Removes a pivot from the map, returning the value at the pivot if the pivot
    /// was previously in the map.
    #[inline]
    pub fn remove(&mut self, search: I) -> Option<V> {
        let mut cursor = unsafe { CursorMut::uninit(self) };
        cursor.seek(int_from_pivot(search));
        if cursor.range()?.start.to_int().to_raw() <= search.to_int().to_raw() {
            return Some(cursor.remove().1);
        }
        None
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
                let (child, _) = node.value(pos, &self.internal).assume_init_read();
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
        // Drop values. We don't need to modify the pivots since we're about to
        // free the nodes anyways.
        if mem::needs_drop::<V>() {
            let mut iter = self.raw_iter();
            while let Some((_pivot, value_ptr)) = unsafe { iter.next(&self.leaf) } {
                unsafe {
                    value_ptr.drop_in_place();
                }
            }
        }

        // Release all allocated memory
        unsafe {
            self.internal.clear_and_free(&self.alloc);
            self.leaf.clear_and_free(&self.alloc);
        }
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU64;
    use crate::RangeTree;

    #[test]
    fn smokee() {
        let mut tree = RangeTree::<NonZeroU64, u64>::try_new().unwrap();

        for i in 1..2 {
            tree.insert(NonZeroU64::new(i).unwrap()..NonZeroU64::new(i + 1).unwrap(), i).unwrap();
        }

        println!("{:?}", unsafe { tree.root.pivots(&tree.leaf) })
    }
}
