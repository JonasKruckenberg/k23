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
pub use nonmax;

use crate::int::int_from_pivot;

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
    /// - [`nonmax::NonMaxU8`]
    /// - [`nonmax::NonMaxU16`]
    /// - [`nonmax::NonMaxU32`]
    /// - [`nonmax::NonMaxU64`]
    /// - [`nonmax::NonMaxU128`]
    /// - [`nonmax::NonMaxI8`]
    /// - [`nonmax::NonMaxI16`]
    /// - [`nonmax::NonMaxI32`]
    /// - [`nonmax::NonMaxI64`]
    /// - [`nonmax::NonMaxI128`]
    #[allow(private_bounds)]
    type Int: RangeTreeInteger;

    const ZERO: Self;
    const MAX: Self;

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
            height: Height::leaf(),
            root: NodeRef::zero(),
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
        debug_assert_eq!(root, NodeRef::zero());
        self.root = NodeRef::zero();
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
        self.height = Height::leaf();
        self.init_root(root);
    }

    /// Returns `true` if the map contains no elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        if self.height != Height::leaf() {
            return false;
        }
        let first_pivot = unsafe { self.root.pivot(pos!(0), &self.leaf) };
        first_pivot == I::Int::MAX
    }

    /// Returns a reference to the value corresponding to the pivot.
    #[inline]
    pub fn get(&self, search: I) -> Option<&V> {
        let cursor = self.cursor_at(Bound::Included(search));
        cursor.iter().next().map(|(_pivot, value)| value)
    }

    /// Returns a mutable reference to the value corresponding to the pivot.
    #[inline]
    pub fn get_mut(&mut self, search: I) -> Option<&mut V> {
        let cursor = self.cursor_mut_at(Bound::Included(search));
        cursor.into_iter_mut().next().map(|(_pivot, value)| value)
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

    /// Removes a pivot from the map, returning the value at the pivot if the pivot
    /// was previously in the map.
    #[inline]
    pub fn remove(&mut self, pivot: I) -> Option<V> {
        let mut cursor = unsafe { CursorMut::uninit(self) };
        cursor.seek(int_from_pivot(pivot));
        if cursor.range()?.end.to_int() == pivot.to_int() {
            return Some(cursor.remove().1);
        }
        None
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
