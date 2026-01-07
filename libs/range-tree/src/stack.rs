use core::marker::PhantomData;
use core::ops::{Index, IndexMut};

use crate::idx::Idx;
use crate::node::{MAX_POOL_SIZE, NodePos, NodeRef, leaf_node_layout, marker};

/// Returns the worst case maximum height for a tree with key `I`.
#[inline]
pub(crate) const fn max_height<I: Idx>() -> usize {
    let (layout,_,_,_) = leaf_node_layout::<I, ()>();
    
    // Get the maximum possible number of leaf nodes, assuming an empty `V`.
    //
    // `NodePool` stores offsets in a u32 and therefore the total pool size
    // cannot exceed `u32::MAX`.
    let mut nodes = MAX_POOL_SIZE / layout.size();
    let mut height = 0;

    // If there are multiple nodes at this height then we need another level
    // above it.
    while nodes > 1 {
        height += 1;

        // If there are less than B nodes then we just need a single root node
        // above it which will never get split.
        if nodes < I::B {
            break;
        }

        // Otherwise assume a worst case with all internal nodes being half-full.
        nodes = nodes.div_ceil(I::B / 2);
    }

    height
}

/// A height in the tree.
///
/// This has the invariant of always being less than `max_height::<I>()`, which
/// allows safe unchecked indexing in a stack.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Height<I: Idx> {
    pub height: usize,
    _m: PhantomData<fn() -> I>,
}

impl<I: Idx> Height<I> {
    /// Returns the height for leaf nodes.
    pub(crate) const LEAF: Self = Self {
        height: 0,
        _m: PhantomData,
    };

    /// Returns the maximum possible height for a tree.
    pub(crate) const MAX: Self = Self {
        height: max_height::<I>(),
        _m: PhantomData,
    };

    /// Returns one level down (towards the leaves).
    #[inline]
    pub(crate) fn down(self) -> Option<Self> {
        if self.height == 0 {
            None
        } else {
            Some(Self {
                height: self.height - 1,
                _m: PhantomData,
            })
        }
    }

    /// Returns one level up (towards the root) up to the given maximum heigh.
    #[inline]
    pub(crate) fn up(self, max: Self) -> Option<Self> {
        if self.height >= max.height {
            None
        } else {
            Some(Self {
                height: self.height + 1,
                _m: PhantomData,
            })
        }
    }
}

pub(crate) struct Stack<I: Idx, V, const H: usize> {
    entries: [(NodeRef<marker::LeafOrInternal<V>>, NodePos<I>); H],
}

impl<I: Idx, V, const H: usize> Clone for Stack<I, V, H> {
    fn clone(&self) -> Self {
        Self { entries: self.entries } 
    }
}

impl<I: Idx, V, const H: usize> Default for Stack<I, V, H> {
    #[inline]
    fn default() -> Self {
        Self {
            // The values here don't matter and zero initialization is slightly
            // faster.
            entries: [(NodeRef::ZERO, NodePos::ZERO); H],
        }
    }
}

impl<I: Idx, V, const H: usize> Index<Height<I>> for Stack<I, V, H> {
    type Output = (NodeRef<marker::LeafOrInternal<V>>, NodePos<I>);

    #[inline]
    fn index(&self, index: Height<I>) -> &Self::Output {
        const { assert!(H == max_height::<I>()) };
        unsafe { self.entries.get_unchecked(index.height) }
    }
}

impl<I: Idx, V, const H: usize> IndexMut<Height<I>> for Stack<I, V, H> {
    #[inline]
    fn index_mut(&mut self, index: Height<I>) -> &mut Self::Output {
        const { assert!(H == max_height::<I>()) };
        unsafe { self.entries.get_unchecked_mut(index.height) }
    }
}
