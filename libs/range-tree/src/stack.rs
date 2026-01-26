//! Stack used for mutable tree operations that records a path through the tree.

use core::marker::PhantomData;
use core::ops::{Index, IndexMut};

use crate::node::{MAX_POOL_SIZE, NodePos, node_layout};
use crate::{NodeRef, RangeTreeInteger};

/// Returns the worst case maximum height for a tree with pivot `I`.
#[inline]
pub(crate) const fn max_height<I: RangeTreeInteger>() -> usize {
    // Get the maximum possible number of leaf nodes, assuming an empty `V`.
    //
    // `NodePool` stores offsets in a u32 and therefore the total pool size
    // cannot exceed `u32::MAX`.
    let mut nodes = MAX_POOL_SIZE / node_layout::<I, ()>().0.size();
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
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Height<I: RangeTreeInteger> {
    height: usize,
    marker: PhantomData<fn() -> I>,
}

impl<I: RangeTreeInteger> PartialEq for Height<I> {
    fn eq(&self, other: &Self) -> bool {
        self.height == other.height
    }
}

impl<I: RangeTreeInteger> Eq for Height<I> {}

impl<I: RangeTreeInteger> Height<I> {
    /// Returns the height for leaf nodes.
    pub(crate) const LEAF: Self = Self {
        height: 0,
        marker: PhantomData,
    };

    /// Returns the maximum possible height for a tree.
    pub(crate) const MAX: Self = Self {
        height: max_height::<I>(),
        marker: PhantomData,
    };

    /// Returns one level down (towards the leaves).
    #[inline]
    pub(crate) const fn down(self) -> Option<Self> {
        if self.height == 0 {
            None
        } else {
            Some(Self {
                height: self.height - 1,
                marker: PhantomData,
            })
        }
    }

    /// Returns one level up (towards the root) up to the given maximum height.
    #[inline]
    pub(crate) const fn up(self, max: Height<I>) -> Option<Self> {
        if self.height >= max.height {
            None
        } else {
            Some(Self {
                height: self.height + 1,
                marker: PhantomData,
            })
        }
    }
}

/// Stack which holds the path to a leaf node from the root of the tree.
///
/// The is large enough to hold `max_height::<I>()`, which depends on the branching
/// factor and the node size.
///
/// The stack is indexed with `Height` which allows unchecked indexing since
/// all heights must be less than `max_height::<I>()`.
#[derive(Clone)]
pub(crate) struct Stack<I: RangeTreeInteger, const H: usize> {
    entries: [(NodeRef, NodePos<I>); H],
}

impl<I: RangeTreeInteger, const H: usize> Default for Stack<I, H> {
    #[inline]
    fn default() -> Self {
        Self {
            // The values here don't matter and zero initialization is slightly
            // faster.
            entries: [(NodeRef::ZERO, NodePos::ZERO); H],
        }
    }
}

impl<I: RangeTreeInteger, const H: usize> Index<Height<I>> for Stack<I, H> {
    type Output = (NodeRef, NodePos<I>);

    #[inline]
    fn index(&self, index: Height<I>) -> &Self::Output {
        const { assert!(H == max_height::<I>()) };
        unsafe { self.entries.get_unchecked(index.height) }
    }
}

impl<I: RangeTreeInteger, const H: usize> IndexMut<Height<I>> for Stack<I, H> {
    #[inline]
    fn index_mut(&mut self, index: Height<I>) -> &mut Self::Output {
        const { assert!(H == max_height::<I>()) };
        unsafe { self.entries.get_unchecked_mut(index.height) }
    }
}
