use alloc::alloc::Global;
use core::alloc::Allocator;
use core::hint;
use core::iter::FusedIterator;
use core::mem::{self, ManuallyDrop};
use core::ops::{Bound, RangeBounds};
use core::ptr::NonNull;
use core::range::RangeInclusive;

use crate::int::{RangeTreeInteger, int_from_pivot, pivot_from_int};
use crate::node::{NodePool, NodePos, NodeRef};
use crate::{RangeTree, RangeTreeIndex};

/// Common base for mutable and immutable iterators.
#[derive(Clone)]
pub(crate) struct RawIter<I: RangeTreeInteger> {
    /// Current leaf node.
    pub(crate) node: NodeRef,

    /// Current position in the node.
    ///
    /// This must point to a valid entry *except* if the iterator has reached
    /// the end of the tree, in which case it must point to the first `Int::MAX`
    /// pivot in the node.
    pub(crate) pos: NodePos<I>,
}

impl<I: RangeTreeInteger> RawIter<I> {
    /// Returns `true` if the iterator points to the end of the tree.
    ///
    /// # Safety
    ///
    /// `leaf_pool` must point to the `NodePool` for leaf nodes in the tree.
    #[inline]
    unsafe fn is_end<V>(&self, leaf_pool: &NodePool<I, V>) -> bool {
        // Safety: ensured by caller
        unsafe { self.node.pivot(self.pos, leaf_pool) == I::MAX }
    }

    /// Returns the next pivot that the iterator will yield, or `I::MAX` if it is
    /// at the end of the tree.
    ///
    /// # Safety
    ///
    /// `leaf_pool` must point to the `NodePool` for leaf nodes in the tree.
    #[inline]
    unsafe fn next_pivot<V>(&self, leaf_pool: &NodePool<I, V>) -> I::Raw {
        // Safety: ensured by caller
        unsafe { self.node.pivot(self.pos, leaf_pool) }
    }

    /// Advances the iterator to the next element in the tree.
    ///
    /// # Safety
    ///
    /// `leaf_pool` must point to the `NodePool` for leaf nodes in the tree.
    #[inline]
    pub(crate) unsafe fn next<V>(&mut self, leaf_pool: &NodePool<I, V>) -> Option<(I, NonNull<V>)> {
        // Safety: iterators only operate on leaf nodes
        unsafe {
            // Get the current element that will be returned.
            let pivot = I::from_raw(self.node.pivot(self.pos, leaf_pool))?;
            let value = self.node.values_ptr(leaf_pool).add(self.pos.index());

            // First, try to move to the next element in the current leaf.
            self.pos = self.pos.next();

            // If we reached the end of the leaf then we need to advance to the next
            // leaf node.
            // Safety: iterators only operate on leaf nodes
            if self.is_end(leaf_pool) {
                // If we've reached the end of the tree then we can leave the
                // iterator pointing to an `Int::MAX` pivot.
                if let Some(next_leaf) = self.node.next_leaf(leaf_pool) {
                    self.node = next_leaf;
                    self.pos = NodePos::ZERO;

                    // Safety: The tree invariants guarantee that leaf nodes are always at least
                    // half full, except if this is the root node. However this can't be the
                    // root node since there is more than one node.
                    hint::assert_unchecked(!self.is_end(leaf_pool));
                }
            }

            Some((pivot, value.cast()))
        }
    }
}

/// An iterator over the entries of a [`RangeTree`].
pub struct Iter<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    pub(crate) raw: RawIter<I::Int>,
    pub(crate) tree: &'a RangeTree<I, V, A>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Iterator for Iter<'a, I, V, A> {
    type Item = (RangeInclusive<I>, &'a V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // Safety: iterators only operate on leaf nodes
        unsafe {
            self.raw.next(&self.tree.leaf).map(|(end, value)| {
                let (start, value) = value.as_ref();

                let range = RangeInclusive {
                    start: *start,
                    end: I::from_int(end),
                };
                (range, value)
            })
        }
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> FusedIterator for Iter<'a, I, V, A> {}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Clone for Iter<'a, I, V, A> {
    fn clone(&self) -> Self {
        Self {
            raw: self.raw.clone(),
            tree: self.tree,
        }
    }
}

/// A mutable iterator over the entries of a [`RangeTree`].
pub struct IterMut<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    pub(crate) raw: RawIter<I::Int>,
    pub(crate) tree: &'a mut RangeTree<I, V, A>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Iterator for IterMut<'a, I, V, A> {
    type Item = (RangeInclusive<I>, &'a mut V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // Safety: iterators only operate on leaf nodes
        unsafe {
            self.raw.next(&self.tree.leaf).map(|(end, mut value)| {
                let (start, value) = value.as_mut();

                let range = RangeInclusive {
                    start: *start,
                    end: I::from_int(end),
                };
                (range, value)
            })
        }
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> FusedIterator for IterMut<'a, I, V, A> {}

/// An owning iterator over the entries of a [`RangeTree`].
pub struct IntoIter<I: RangeTreeIndex, V, A: Allocator = Global> {
    raw: RawIter<I::Int>,
    tree: ManuallyDrop<RangeTree<I, V, A>>,
}

impl<I: RangeTreeIndex, V, A: Allocator> Iterator for IntoIter<I, V, A> {
    type Item = (RangeInclusive<I>, V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // Read the element out of the tree without touching the pivot.
        // Safety: iterators only operate on leaf nodes
        unsafe {
            self.raw.next(&self.tree.leaf).map(|(end, value)| {
                let (start, value) = value.read();

                let range = RangeInclusive {
                    start,
                    end: I::from_int(end),
                };
                (range, value)
            })
        }
    }
}

impl<I: RangeTreeIndex, V, A: Allocator> Drop for IntoIter<I, V, A> {
    #[inline]
    fn drop(&mut self) {
        // Ensure all remaining elements are dropped.
        if mem::needs_drop::<V>() {
            // Safety: `IntoIter` was created from a `RangeTree` so the allocators always match
            while let Some((_pivot, value_ptr)) = unsafe { self.raw.next(&self.tree.leaf) } {
                // Safety: `RawIter` yields only entries where `pivot` is non-max, meaning the value
                // is present and initialized.
                unsafe {
                    value_ptr.drop_in_place();
                }
            }
        }

        // Then release the allocations for the tree without dropping elements.
        // Safety: `IntoIter` was created from a `RangeTree` so the allocators always match
        unsafe {
            let tree = &mut *self.tree;
            tree.internal.clear_and_free(&tree.alloc);
            tree.leaf.clear_and_free(&tree.alloc);
        }
    }
}

impl<I: RangeTreeIndex, V, A: Allocator> FusedIterator for IntoIter<I, V, A> {}

/// An iterator over the pivots of a [`RangeTree`].
pub struct Ranges<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    raw: RawIter<I::Int>,
    tree: &'a RangeTree<I, V, A>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Iterator for Ranges<'a, I, V, A> {
    type Item = RangeInclusive<I>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // Safety: iterators only operate on leaf nodes
        unsafe {
            self.raw.next(&self.tree.leaf).map(|(end, value)| {
                let (start, _) = value.as_ref();

                RangeInclusive {
                    start: *start,
                    end: I::from_int(end),
                }
            })
        }
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> FusedIterator for Ranges<'a, I, V, A> {}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Clone for Ranges<'a, I, V, A> {
    fn clone(&self) -> Self {
        Self {
            raw: self.raw.clone(),
            tree: self.tree,
        }
    }
}

/// An iterator over the values of a [`RangeTree`].
pub struct Values<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    raw: RawIter<I::Int>,
    tree: &'a RangeTree<I, V, A>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Iterator for Values<'a, I, V, A> {
    type Item = &'a V;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // Safety: iterators only operate on leaf nodes
        unsafe {
            self.raw.next(&self.tree.leaf).map(|(_pivot, value_ptr)| {
                let (_, value) = value_ptr.as_ref();
                value
            })
        }
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> FusedIterator for Values<'a, I, V, A> {}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Clone for Values<'a, I, V, A> {
    fn clone(&self) -> Self {
        Self {
            raw: self.raw.clone(),
            tree: self.tree,
        }
    }
}

/// A mutable iterator over the values of a [`RangeTree`].
pub struct ValuesMut<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    raw: RawIter<I::Int>,
    tree: &'a mut RangeTree<I, V, A>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Iterator for ValuesMut<'a, I, V, A> {
    type Item = &'a mut V;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // Safety: iterators only operate on leaf nodes
        unsafe {
            self.raw
                .next(&self.tree.leaf)
                .map(|(_pivot, mut value_ptr)| {
                    let (_, value) = value_ptr.as_mut();
                    value
                })
        }
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> FusedIterator for ValuesMut<'a, I, V, A> {}

/// An iterator over a sub-range of the entries of a [`RangeTree`].
pub struct Range<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    raw: RawIter<I::Int>,
    end: <I::Int as RangeTreeInteger>::Raw,
    tree: &'a RangeTree<I, V, A>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Iterator for Range<'a, I, V, A> {
    type Item = (RangeInclusive<I>, &'a V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // Safety: iterators only operate on leaf nodes
        unsafe {
            if I::Int::cmp(self.raw.next_pivot(&self.tree.leaf), self.end).is_ge() {
                return None;
            }

            self.raw.next(&self.tree.leaf).map(|(end, value)| {
                let (start, value) = value.as_ref();

                let range = RangeInclusive {
                    start: *start,
                    end: I::from_int(end),
                };
                (range, value)
            })
        }
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> FusedIterator for Range<'a, I, V, A> {}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Clone for Range<'a, I, V, A> {
    fn clone(&self) -> Self {
        Self {
            raw: self.raw.clone(),
            end: self.end,
            tree: self.tree,
        }
    }
}

/// A mutable iterator over a sub-range of the entries of a [`RangeTree`].
pub struct RangeMut<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    raw: RawIter<I::Int>,
    end: <I::Int as RangeTreeInteger>::Raw,
    tree: &'a mut RangeTree<I, V, A>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Iterator for RangeMut<'a, I, V, A> {
    type Item = (RangeInclusive<I>, &'a mut V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // Safety: iterators only operate on leaf nodes
        unsafe {
            if I::Int::cmp(self.raw.next_pivot(&self.tree.leaf), self.end).is_ge() {
                return None;
            }

            self.raw.next(&self.tree.leaf).map(|(end, mut value)| {
                let (start, value) = value.as_mut();

                let range = RangeInclusive {
                    start: *start,
                    end: I::from_int(end),
                };
                (range, value)
            })
        }
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> FusedIterator for RangeMut<'a, I, V, A> {}

/// An iterator over gaps between the ranges of a [`RangeTree`].
pub struct Gaps<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    inner: Ranges<'a, I, V, A>,
    prev_end: Option<Bound<I>>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Iterator for Gaps<'a, I, V, A> {
    type Item = (Bound<I>, Bound<I>);

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(prev_end) = self.prev_end.take() {
            let gap = if let Some(range) = self.inner.next() {
                let gap = (prev_end, Bound::Excluded(range.start));

                self.prev_end = pivot_from_int(I::Int::increment(int_from_pivot(range.end)))
                    .map(Bound::Included);

                gap
            } else {
                (prev_end, Bound::Unbounded)
            };

            // if this gap is NOT empty, yield it
            if !is_empty(gap) {
                return Some(gap);
            }
        }

        None
    }
}

fn is_empty<I>(gap: impl RangeBounds<I>) -> bool
where
    I: RangeTreeIndex,
{
    match (gap.start_bound(), gap.end_bound()) {
        (Bound::Unbounded, _) | (_, Bound::Unbounded) => false,
        (Bound::Included(start), Bound::Excluded(end)) => {
            I::Int::cmp(start.to_int().to_raw(), end.to_int().to_raw()).is_ge()
        }
        _ => unreachable!(),
    }
}

impl<I: RangeTreeIndex, V, A: Allocator> RangeTree<I, V, A> {
    /// Returns a [`RawIter`] pointing at the first element of the tree.
    #[inline]
    pub(crate) fn raw_iter(&self) -> RawIter<I::Int> {
        // The first leaf node is always the left-most leaf on the tree and is
        // never deleted.
        let node = NodeRef::ZERO;
        let pos = pos!(0);
        RawIter { node, pos }
    }

    /// Returns a [`RawIter`] pointing at the first element with pivot greater
    /// than or equal to `pivot`.
    #[inline]
    pub(crate) fn raw_iter_from(
        &self,
        search: <I::Int as RangeTreeInteger>::Raw,
    ) -> RawIter<I::Int> {
        // Go down the tree, at each internal node selecting the first sub-tree
        // with pivot greater than or equal to the search pivot. This sub-tree will
        // only contain pivots less than or equal to its pivot.
        let mut height = self.height;
        let mut node = self.root;
        while let Some(down) = height.down() {
            // Safety: `height > LEAF` so this MUST be an internal node
            let pivots = unsafe { node.pivots(&self.internal) };
            // Safety: ensured by `RangeTreeInteger` trait
            let pos = unsafe { I::Int::search(pivots, search) };
            // Safety: `height > LEAF` so this MUST be an internal node AND `pos` returned by `search`
            // points to an initialized entry.
            node = unsafe { node.value(pos, &self.internal).assume_init_read().0 };
            height = down;
        }

        // Select the first leaf element with pivot greater than or equal to the
        // search pivot.
        // Safety: we reached `height == LEAF` so this MUST be a leaf
        let pivots = unsafe { node.pivots(&self.leaf) };
        // Safety: ensured by `RangeTreeInteger` trait
        let pos = unsafe { I::Int::search(pivots, search) };
        RawIter { node, pos }
    }

    /// Gets an iterator over the entries of the map, sorted by pivot.
    #[inline]
    pub fn iter(&self) -> Iter<'_, I, V, A> {
        Iter {
            raw: self.raw_iter(),
            tree: self,
        }
    }

    /// Gets a mutable iterator over the entries of the map, sorted by pivot.
    #[inline]
    pub fn iter_mut(&mut self) -> IterMut<'_, I, V, A> {
        IterMut {
            raw: self.raw_iter(),
            tree: self,
        }
    }

    /// Gets an iterator over the entries of the map starting from the given
    /// bound.
    #[inline]
    pub fn iter_from(&self, bound: Bound<I>) -> Iter<'_, I, V, A> {
        let raw = match bound {
            Bound::Included(pivot) => self.raw_iter_from(int_from_pivot(pivot)),
            Bound::Excluded(pivot) => self.raw_iter_from(I::Int::increment(int_from_pivot(pivot))),
            Bound::Unbounded => self.raw_iter(),
        };
        Iter { raw, tree: self }
    }

    /// Gets a mutable iterator over the entries of the map starting from the
    /// given bound.
    #[inline]
    pub fn iter_mut_from(&mut self, bound: Bound<I>) -> IterMut<'_, I, V, A> {
        let raw = match bound {
            Bound::Included(pivot) => self.raw_iter_from(int_from_pivot(pivot)),
            Bound::Excluded(pivot) => self.raw_iter_from(I::Int::increment(int_from_pivot(pivot))),
            Bound::Unbounded => self.raw_iter(),
        };
        IterMut { raw, tree: self }
    }

    /// Gets an iterator over the pivots of the map, in sorted order.
    #[inline]
    pub fn ranges(&self) -> Ranges<'_, I, V, A> {
        Ranges {
            raw: self.raw_iter(),
            tree: self,
        }
    }

    /// Gets an iterator over the values of the map, in order by pivot.
    #[inline]
    pub fn values(&self) -> Values<'_, I, V, A> {
        Values {
            raw: self.raw_iter(),
            tree: self,
        }
    }

    /// Gets a mutable iterator over the values of the map, in order by pivot.
    #[inline]
    pub fn values_mut(&mut self) -> ValuesMut<'_, I, V, A> {
        ValuesMut {
            raw: self.raw_iter(),
            tree: self,
        }
    }

    /// Constructs an iterator over a sub-range of elements in the map.
    ///
    /// Unlike `BTreeMap`, this is not a [`DoubleEndedIterator`]: it only allows
    /// forward iteration.
    #[inline]
    pub fn range(&self, range: impl RangeBounds<I>) -> Range<'_, I, V, A> {
        let raw = match range.start_bound() {
            Bound::Included(&pivot) => self.raw_iter_from(int_from_pivot(pivot)),
            Bound::Excluded(&pivot) => self.raw_iter_from(I::Int::increment(int_from_pivot(pivot))),
            Bound::Unbounded => self.raw_iter(),
        };
        let end = match range.end_bound() {
            Bound::Included(&pivot) => I::Int::increment(int_from_pivot(pivot)),
            Bound::Excluded(&pivot) => int_from_pivot(pivot),
            Bound::Unbounded => I::Int::MAX,
        };
        Range {
            raw,
            end,
            tree: self,
        }
    }

    /// Constructs a mutable iterator over a sub-range of elements in the map.
    ///
    /// Unlike `BTreeMap`, this is not a [`DoubleEndedIterator`]: it only allows
    /// forward iteration.
    #[inline]
    pub fn range_mut(&mut self, range: impl RangeBounds<I>) -> RangeMut<'_, I, V, A> {
        let raw = match range.start_bound() {
            Bound::Included(&pivot) => self.raw_iter_from(int_from_pivot(pivot)),
            Bound::Excluded(&pivot) => self.raw_iter_from(I::Int::increment(int_from_pivot(pivot))),
            Bound::Unbounded => self.raw_iter(),
        };
        let end = match range.end_bound() {
            Bound::Included(&pivot) => I::Int::increment(int_from_pivot(pivot)),
            Bound::Excluded(&pivot) => int_from_pivot(pivot),
            Bound::Unbounded => I::Int::MAX,
        };
        RangeMut {
            raw,
            end,
            tree: self,
        }
    }

    /// An iterator over gaps between the ranges of a [`RangeTree`].
    ///
    /// Always yields range bounds like this:
    /// - The first pair is always `(Bound::Unbounded, _)` to indicate the gap _before_ all ranges.
    /// - The last pair is always `(_, Bound::Unbounded)` to indicate the gap _after_ all ranges.
    /// - All pairs in between are of shape `(Bound::Included, Bound::Excluded)`.
    pub fn gaps(&self) -> Gaps<'_, I, V, A> {
        Gaps {
            inner: self.ranges(),
            prev_end: Some(Bound::Unbounded),
        }
    }
}

impl<I: RangeTreeIndex, V, A: Allocator> IntoIterator for RangeTree<I, V, A> {
    type Item = (RangeInclusive<I>, V);

    type IntoIter = IntoIter<I, V, A>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            raw: self.raw_iter(),
            tree: ManuallyDrop::new(self),
        }
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> IntoIterator for &'a RangeTree<I, V, A> {
    type Item = (RangeInclusive<I>, &'a V);

    type IntoIter = Iter<'a, I, V, A>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> IntoIterator for &'a mut RangeTree<I, V, A> {
    type Item = (RangeInclusive<I>, &'a mut V);

    type IntoIter = IterMut<'a, I, V, A>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}
