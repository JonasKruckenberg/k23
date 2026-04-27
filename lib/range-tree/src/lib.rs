//! This crate provides [`RangeTree`], a fast B+ Tree implementation storing integer ranges.

#![cfg_attr(not(test), no_std)]
#![feature(allocator_api)]
#![feature(new_range_api)]
#![warn(missing_docs)]
#![expect(
    clippy::undocumented_unsafe_blocks,
    reason = "all uses on unsafe are checked an mostly documented. Adding more safety comments would just hurt readability."
)]

extern crate alloc;

#[macro_use]
mod node;

mod cursor;
mod int;
mod iter;
mod simd;
mod stack;

use alloc::alloc::Global;
use core::alloc::{AllocError, Allocator};
use core::ops::Bound;
use core::{fmt, mem, range};

pub use cursor::*;
use int::RangeTreeInteger;
pub use iter::*;
use node::{NodePool, NodeRef, UninitNodeRef};
use stack::Height;

use crate::int::int_from_pivot;
use crate::node::NodePos;

/// Error indicating range overlaps with an existing range in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlapError;

impl fmt::Display for OverlapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "overlapping range")
    }
}

/// Trait which must be implemented for all range indices inserted into a [`RangeTree`].
///
/// [`RangeTree`] requires that range indices be integers and reserves the ZERO `0` value
/// for internal use. This trait is already implemented for all unsigned nonzero integers,
/// but this crate allows for custom pivot types that are convertible to/from those integers.
///
/// Note that pivots in the [`RangeTree`] are ordered by their integer value and not
/// the [`Ord`] implementation of the pivot type.
pub trait RangeTreeIndex: Copy {
    /// Non-zero integer type that this index maps to.
    ///
    /// This must be one of the `NonZero` integer types:
    /// - [`core::num::NonZeroU8`]
    /// - [`core::num::NonZeroU16`]
    /// - [`core::num::NonZeroU32`]
    /// - [`core::num::NonZeroU64`]
    /// - [`core::num::NonZeroU128`]
    #[allow(
        private_bounds,
        reason = "this is fine, callers should not be able to implement `RangeTreeInteger`"
    )]
    type Int: RangeTreeInteger;

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
    ///
    /// # Errors
    ///
    /// Returns `Err(AllocError)` if allocating the initial node of the tree failed.
    #[inline]
    pub fn try_new() -> Result<Self, AllocError> {
        Self::try_new_in(Global)
    }
}

impl<I: RangeTreeIndex, V, A: Allocator> RangeTree<I, V, A> {
    /// Creates a new, empty [`RangeTree`] with the given allocator.
    ///
    /// This requires an initial memory allocation on creation.
    ///
    /// # Errors
    ///
    /// Returns `Err(AllocError)` if allocating the initial node of the tree failed.
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

        // Safety: we allocated `root` from the leaf node pool above
        unsafe {
            out.init_root(root);
        }

        Ok(out)
    }

    /// Initializes the root node to the leaf node at offset zero.
    ///
    /// # Safety
    ///
    /// `root` must be allocated from the `NodePool` for leaf nodes.
    #[inline]
    unsafe fn init_root(&mut self, root: UninitNodeRef) {
        // Safety: ensured by caller
        unsafe {
            let root = root.init_pivots(&mut self.leaf);
            root.set_next_leaf(None, &mut self.leaf);
            debug_assert_eq!(root, NodeRef::ZERO);
        }

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
                // Safety: `RawIter` yields only entries where `pivot` is non-max, meaning the value
                // is present and initialized.
                unsafe {
                    value_ptr.drop_in_place();
                }
            }
        }

        // Free all nodes without freeing the underlying allocations.
        self.internal.clear();
        let root = self.leaf.clear_and_alloc_node();

        // Re-initialize the root node.
        self.height = Height::LEAF;

        // Safety: we allocated `root` from the leaf node pool above
        unsafe {
            self.init_root(root);
        }
    }

    /// Returns `true` if the map contains no elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        if self.height == Height::LEAF {
            // Safety: if the tree height is `LEAF` (which we tested for above) the root MUST be
            // a leaf node
            let first_pivot = unsafe { self.root.pivot(pos!(0), &self.leaf) };
            first_pivot == I::Int::MAX
        } else {
            // if we do have internal nodes that means we have split, meaning the tree cannot be
            // empty
            false
        }
    }

    /// Returns a reference to the value corresponding to the pivot.
    #[inline]
    pub fn get(&self, search: I) -> Option<&V> {
        let cursor = self.cursor_at(Bound::Included(search));
        let (range, value) = cursor.iter().next()?;

        if I::Int::cmp(range.start.to_int().to_raw(), search.to_int().to_raw()).is_le() {
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

        if I::Int::cmp(range.start.to_int().to_raw(), search.to_int().to_raw()).is_le() {
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
    ///
    /// Inserts a range and associated value into the map.
    ///
    /// # Errors
    ///
    /// If the entry could not be inserted, either because allocation failed
    ///
    /// Returns `Err` when insertion fails either because allocating the required memory failed
    /// or because the
    #[inline]
    pub fn insert(
        &mut self,
        range: impl Into<range::RangeInclusive<I>>,
        value: V,
    ) -> Result<(), OverlapError> {
        // TODO remove this once `new_range_api` is stable.
        let range = range.into();

        // Safety: we immediately initialize the cursor below
        let mut cursor = unsafe { CursorMut::uninit(self) };
        cursor.seek(int_from_pivot(range.end));

        if let Some((existing, _)) = cursor.entry()
            && I::Int::cmp(
                existing.start.to_int().to_raw(),
                range.end.to_int().to_raw(),
            )
            .is_lt()
        {
            return Err(OverlapError);
        }

        if cursor.prev() {
            if let Some((prev, _)) = cursor.entry()
                && I::Int::cmp(prev.end.to_int().to_raw(), range.start.to_int().to_raw()).is_gt()
            {
                // Overlap detected: previous range ends after new range starts
                return Err(OverlapError);
            }

            cursor.next(); // Move back to insertion position
        }

        cursor.insert(range, value);

        Ok(())
    }

    /// Removes a pivot from the map, returning the value at the pivot if the pivot
    /// was previously in the map.
    #[inline]
    pub fn remove(&mut self, search: I) -> Option<V> {
        // Safety: we immediately initialize the cursor below
        let mut cursor = unsafe { CursorMut::uninit(self) };
        cursor.seek(int_from_pivot(search));

        if I::Int::cmp(
            cursor.range()?.start.to_int().to_raw(),
            search.to_int().to_raw(),
        )
        .is_le()
        {
            Some(cursor.remove().1)
        } else {
            None
        }
    }

    /// Assert as many invariants about the tree as possible
    ///
    /// # Panics
    ///
    /// Will panic if any invariant is violated.
    pub fn assert_valid(&self) {
        let mut last_leaf = None;
        self.check_node(
            self.root,
            self.height,
            true,
            None,
            I::Int::MAX,
            &mut last_leaf,
        );

        // Ensure the linked list of leaf nodes is properly terminated.
        // Safety: `last_leaf` is only updated with leaf NodeRefs
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
            // Safety: we have checked this node to be at leaf-level. It MUST be a leaf node.
            unsafe {
                self.check_leaf_node(node, assert_sorted, min, max, prev_leaf);
            }
            return;
        };

        let keys = || {
            (0..I::Int::B).map(|i| {
                // Safety: `0..I::B` only produces indices `< I::B`
                let pos = unsafe { NodePos::new_unchecked(i) };

                // Safety: all leaf nodes are handled above, therefore it MUST be an internal node
                unsafe { node.pivot(pos, &self.internal) }
            })
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

        assert!(len < I::Int::B);
        // Check the invariants for child nodes.
        let mut prev_key = min;
        for i in 0..len {
            // Safety: `len` is the number of all pivots `!= MAX`
            //  => `len` must be `< B`
            //  => every `i..len` must be a valid position
            //  => every position `i` must be valid for reads
            // Additionally, all leaf nodes are handled at the top of the functions,
            // therefore this MUST be an internal node
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

    /// Assert that `node` is a valid leaf node.
    ///
    /// # Safety
    ///
    /// `node` must be allocated from the NodePool for leaf nodes in the tree.
    unsafe fn check_leaf_node(
        &self,
        node: NodeRef,
        assert_sorted: bool,
        min: Option<<I::Int as RangeTreeInteger>::Raw>,
        max: <I::Int as RangeTreeInteger>::Raw,
        prev_leaf: &mut Option<NodeRef>,
    ) {
        let keys = || {
            (0..I::Int::B).map(|i| {
                // Safety: `0..I::B` only produces indices `< I::B`
                let pos = unsafe { NodePos::new_unchecked(i) };

                // Safety: ensured by caller
                unsafe { node.pivot(pos, &self.leaf) }
            })
        };

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
            // Safety: ensured by caller.
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
                // Safety: `RawIter` yields only entries where `pivot` is non-max, meaning the value
                // is present and initialized.
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
