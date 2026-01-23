use core::alloc::Allocator;
use core::{hint, ops};
use core::iter::FusedIterator;
use core::marker::PhantomData;
use core::ptr::NonNull;
use crate::int::RangeTreeInteger;
use crate::node::{marker, NodePool, NodePos, NodeRef};
use crate::{RangeTreeIndex, RangeTree};

/// Common base for mutable and immutable iterators.
pub(crate) struct RawIter<I: RangeTreeIndex, V> {
    /// Current leaf node.
    pub(crate) node: NodeRef,

    /// Current position in the node.
    ///
    /// This must point to a valid entry *except* if the iterator has reached
    /// the end of the tree, in which case it must point to the first `Int::MAX`
    /// key in the node.
    pub(crate) pos: NodePos<I::Int>,
    
    _value: PhantomData<V>
}

impl<I: RangeTreeIndex, V> Clone for RawIter<I, V> {
    fn clone(&self) -> Self {
        Self { node: self.node.clone(), pos: self.pos.clone(), _value: PhantomData }
    }
}

impl<I: RangeTreeIndex, V> RawIter<I, V> {
    /// Returns `true` if the iterator points to the end of the tree.
    ///
    /// # Safety
    ///
    /// `leaf_pool` must point to the `NodePool` for leaf nodes in the tree.
    #[inline]
    unsafe fn is_end(&self, leaf_pool: &NodePool<I::Int, marker::Leaf<V>>) -> bool {
        unsafe { self.node.pivot(self.pos, leaf_pool) == I::Int::MAX }
    }

    /// Returns the next key that the iterator will yield, or `I::MAX` if it is
    /// at the end of the tree.
    ///
    /// # Safety
    ///
    /// `leaf_pool` must point to the `NodePool` for leaf nodes in the tree.
    #[inline]
    unsafe fn next_key(&self, leaf_pool: &NodePool<I::Int, marker::Leaf<V>>) -> <I::Int as RangeTreeInteger>::Raw {
        unsafe { self.node.pivot(self.pos, leaf_pool) }
    }

    /// Advances the iterator to the next element in the tree.
    ///
    /// # Safety
    ///
    /// `leaf_pool` must point to the `NodePool` for leaf nodes in the tree.
    #[inline]
    pub(crate) unsafe fn next(&mut self, leaf_pool: &NodePool<I::Int, marker::Leaf<V>>) -> Option<(ops::Range<I>, NonNull<V>)> {
        // Get the current element that will be returned.
        let pivot = unsafe { I::Int::from_raw(self.node.pivot(self.pos, leaf_pool))? };
        let start = unsafe { I::Int::from_raw(self.node.start(self.pos, leaf_pool).assume_init_read()).unwrap() };
        let value = unsafe { self.node.values_ptr(leaf_pool).add(self.pos.index()) };

        // First, try to move to the next element in the current leaf.
        self.pos = unsafe { self.pos.next() };

        // If we reached the end of the leaf then we need to advance to the next
        // leaf node.
        if unsafe { self.is_end(leaf_pool) } {
            // If we've reached the end of the tree then we can leave the
            // iterator pointing to an `Int::MAX` key.
            if let Some(next_leaf) = unsafe { self.node.next_leaf(leaf_pool) } {
                self.node = next_leaf;
                self.pos = NodePos::ZERO;

                // The tree invariants guarantee that leaf nodes are always at least
                // half full, except if this is the root node. However, this can't be the
                // root node since there is more than one node.
                unsafe {
                    hint::assert_unchecked(!self.is_end(leaf_pool));
                }
            }
        }


        Some((I::from_int(start)..I::from_int(pivot), value.cast()))
    }
}

/// An iterator over the keys of a [`BTree`].
pub struct Ranges<'a, I: RangeTreeIndex, V, A: Allocator> {
    raw: RawIter<I, V>,
    btree: &'a RangeTree<I, V, A>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Iterator for Ranges<'a, I, V, A> {
    type Item = ops::Range<I>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            self.raw
                .next(&self.btree.leaf)
                .map(|(key, _value_ptr)| key)
        }
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> FusedIterator for Ranges<'a, I, V, A> {}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Clone for Ranges<'a, I, V, A> {
    fn clone(&self) -> Self {
        Self {
            raw: self.raw.clone(),
            btree: self.btree,
        }
    }
}

/// An iterator over the values of a [`BTree`].
pub struct Values<'a, I: RangeTreeIndex, V, A: Allocator> {
    raw: RawIter<I, V>,
    btree: &'a RangeTree<I, V, A>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Iterator for Values<'a, I, V, A> {
    type Item = &'a V;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            self.raw
                .next(&self.btree.leaf)
                .map(|(_key, value_ptr)| value_ptr.as_ref())
        }
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> FusedIterator for Values<'a, I, V, A> {}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Clone for Values<'a, I, V, A> {
    fn clone(&self) -> Self {
        Self {
            raw: self.raw.clone(),
            btree: self.btree,
        }
    }
}

/// A mutable iterator over the values of a [`BTree`].
pub struct ValuesMut<'a, I: RangeTreeIndex, V, A: Allocator> {
    raw: RawIter<I, V>,
    btree: &'a mut RangeTree<I, V, A>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Iterator for ValuesMut<'a, I, V, A> {
    type Item = &'a mut V;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            self.raw
                .next(&self.btree.leaf)
                .map(|(_key, mut value_ptr)| value_ptr.as_mut())
        }
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> FusedIterator for ValuesMut<'a, I, V, A> {}

impl<I: RangeTreeIndex, V, A: Allocator> RangeTree<I, V, A> {
    #[inline]
    pub(crate) fn raw_iter(&self) -> RawIter<I, V> {
        // The first leaf node is always the left-most leaf on the tree and is
        // never deleted.
        RawIter { node: NodeRef::ZERO, pos: NodePos::ZERO, _value: PhantomData }
    }

    /// Gets an iterator over the ranges of the map, in sorted order.
    #[inline]
    pub fn ranges(&self) -> Ranges<'_, I, V, A> {
        Ranges {
            raw: self.raw_iter(),
            btree: self,
        }
    }

    /// Gets an iterator over the values of the map, in order by key.
    #[inline]
    pub fn values(&self) -> Values<'_, I, V, A> {
        Values {
            raw: self.raw_iter(),
            btree: self,
        }
    }

    /// Gets a mutable iterator over the values of the map, in order by key.
    #[inline]
    pub fn values_mut(&mut self) -> ValuesMut<'_, I, V, A> {
        ValuesMut {
            raw: self.raw_iter(),
            btree: self,
        }
    }
}