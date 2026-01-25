use alloc::alloc::Global;
use core::alloc::Allocator;
use core::iter::FusedIterator;
use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::ptr::NonNull;
use core::{hint, mem, ops};

use crate::int::RangeTreeInteger;
use crate::node::{LeafNodePayload, NodePool, NodePos, NodeRef, marker};
use crate::{RangeTree, RangeTreeIndex};

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

    pub(crate) _value: PhantomData<V>,
}

impl<I: RangeTreeIndex, V> Clone for RawIter<I, V> {
    fn clone(&self) -> Self {
        Self {
            node: self.node.clone(),
            pos: self.pos.clone(),
            _value: PhantomData,
        }
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
    unsafe fn next_key(
        &self,
        leaf_pool: &NodePool<I::Int, marker::Leaf<V>>,
    ) -> <I::Int as RangeTreeInteger>::Raw {
        unsafe { self.node.pivot(self.pos, leaf_pool) }
    }

    /// Advances the iterator to the next element in the tree.
    ///
    /// # Safety
    ///
    /// `leaf_pool` must point to the `NodePool` for leaf nodes in the tree.
    #[inline]
    pub(crate) unsafe fn next(
        &mut self,
        leaf_pool: &NodePool<I::Int, marker::Leaf<V>>,
    ) -> Option<(I, NonNull<LeafNodePayload<I::Int, V>>)> {
        // Get the current element that will be returned.
        let pivot = unsafe { I::Int::from_raw(self.node.pivot(self.pos, leaf_pool))? };
        let payload = unsafe { self.node.payloads_ptr(leaf_pool).add(self.pos.index()) };

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

        Some((I::from_int(pivot), payload.cast()))
    }
}

/// An iterator over the entries of a [`BTree`].
pub struct Iter<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    pub(crate) raw: RawIter<I, V>,
    pub(crate) tree: &'a RangeTree<I, V, A>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Iterator for Iter<'a, I, V, A> {
    type Item = (ops::Range<I>, &'a V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            self.raw.next(&self.tree.leaf).map(|(end, payload)| {
                let payload = payload.as_ref();
                let start = I::from_int(payload.start);

                (start..end, &payload.value)
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

/// A mutable iterator over the entries of a [`BTree`].
pub struct IterMut<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    pub(crate) raw: RawIter<I, V>,
    pub(crate) tree: &'a mut RangeTree<I, V, A>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Iterator for IterMut<'a, I, V, A> {
    type Item = (ops::Range<I>, &'a mut V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            self.raw.next(&self.tree.leaf).map(|(end, mut payload)| {
                let payload = payload.as_mut();
                let start =  I::from_int(payload.start);

                (start..end, &mut payload.value)
            })
        }
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> FusedIterator for IterMut<'a, I, V, A> {}

/// An owning iterator over the entries of a [`BTree`].
pub struct IntoIter<I: RangeTreeIndex, V, A: Allocator = Global> {
    raw: RawIter<I, V>,
    btree: ManuallyDrop<RangeTree<I, V, A>>,
}

impl<I: RangeTreeIndex, V, A: Allocator> Iterator for IntoIter<I, V, A> {
    type Item = (ops::Range<I>, V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        // Read the element out of the tree without touching the key.
        unsafe {
            self.raw.next(&self.btree.leaf).map(|(end, payload)| {
                let payload = payload.read();
                let start =  I::from_int(payload.start);

                (start..end, payload.value)
            })
        }
    }
}

impl<I: RangeTreeIndex, V, A: Allocator> Drop for IntoIter<I, V, A> {
    #[inline]
    fn drop(&mut self) {
        // Ensure all remaining elements are dropped.
        if mem::needs_drop::<V>() {
            while let Some((_key, value_ptr)) = unsafe { self.raw.next(&self.btree.leaf) } {
                unsafe {
                    value_ptr.drop_in_place();
                }
            }
        }

        // Then release the allocations for the tree without dropping elements.
        unsafe {
            let btree = &mut *self.btree;
            btree.internal.clear_and_free(&btree.allocator);
            btree.leaf.clear_and_free(&btree.allocator);
        }
    }
}

impl<I: RangeTreeIndex, V, A: Allocator> FusedIterator for IntoIter<I, V, A> {}

/// An iterator over the keys of a [`BTree`].
pub struct Ranges<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
    raw: RawIter<I, V>,
    btree: &'a RangeTree<I, V, A>,
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> Iterator for Ranges<'a, I, V, A> {
    type Item = ops::Range<I>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            self.raw.next(&self.btree.leaf).map(|(end, payload)| {
                let payload = payload.as_ref();
                let start =  I::from_int(payload.start);

                start..end
            })
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
pub struct Values<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
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
                .map(|(_pivot, value_ptr)| &value_ptr.as_ref().value)
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
pub struct ValuesMut<'a, I: RangeTreeIndex, V, A: Allocator = Global> {
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
                .map(|(_pivot, mut value_ptr)| &mut value_ptr.as_mut().value)
        }
    }
}

impl<'a, I: RangeTreeIndex, V, A: Allocator> FusedIterator for ValuesMut<'a, I, V, A> {}

impl<I: RangeTreeIndex, V, A: Allocator> RangeTree<I, V, A> {
    #[inline]
    pub(crate) fn raw_iter(&self) -> RawIter<I, V> {
        // The first leaf node is always the left-most leaf on the tree and is
        // never deleted.
        RawIter {
            node: NodeRef::ZERO,
            pos: NodePos::ZERO,
            _value: PhantomData,
        }
    }

    /// Gets an iterator over the entries of the map, sorted by key.
    #[inline]
    pub fn iter(&self) -> Iter<'_, I, V, A> {
        Iter {
            raw: self.raw_iter(),
            tree: self,
        }
    }

    /// Gets a mutable iterator over the entries of the map, sorted by key.
    #[inline]
    pub fn iter_mut(&mut self) -> IterMut<'_, I, V, A> {
        IterMut {
            raw: self.raw_iter(),
            tree: self,
        }
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
