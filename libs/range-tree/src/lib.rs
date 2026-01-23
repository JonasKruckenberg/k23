#![cfg_attr(not(test), no_std)]
#![feature(allocator_api)]
extern crate alloc;

mod cursor;
mod int;
mod iter;
mod node;
mod simd;
mod stack;

use core::alloc::{AllocError, Allocator};
use core::ops;

pub use cursor::{Cursor, CursorMut};
use int::RangeTreeInteger;

use crate::node::{NodePool, NodePos, NodeRef, UninitNodeRef, marker};
use crate::stack::Height;

pub trait RangeTreeIndex: Copy {
    #[allow(private_bounds)]
    type Int: RangeTreeInteger;

    /// Converts the index to an integer.
    fn to_int(self) -> Self::Int;

    /// Recovers the index from an integer.
    fn from_int(int: Self::Int) -> Self;
}

pub struct RangeTree<I: RangeTreeIndex, V, A: Allocator> {
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

    #[inline]
    pub fn insert(&mut self, range: ops::Range<I>, value: V) -> Result<(), AllocError> {
        let mut cursor = unsafe { CursorMut::uninit(self) };
        cursor.seek(range.end.to_int().to_raw());
        cursor.insert_before(range, value)
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

#[cfg(test)]
mod tests {
    use alloc::alloc::Global;

    use nonmax::NonMaxU64;
    use proptest::collection::SizeRange;
    use proptest::prelude::*;
    use rand::seq::SliceRandom;

    use super::*;
    use crate::node::{internal_node_layout, leaf_node_layout};

    struct Ranges {
        num_ranges: SizeRange,
        size: ops::Range<u64>,
        gap: ops::Range<u64>,
        shuffled: bool,
    }

    impl Ranges {
        pub fn new(num_ranges: impl Into<SizeRange>) -> Self {
            Self {
                num_ranges: num_ranges.into(),
                size: 0..4096,
                gap: 0..49096,
                shuffled: true,
            }
        }

        pub fn size(mut self, size: ops::Range<u64>) -> Self {
            self.size = size;
            self
        }

        pub fn gap(mut self, gap: ops::Range<u64>) -> Self {
            self.gap = gap;
            self
        }

        pub fn shuffled(mut self, shuffled: bool) -> Self {
            self.shuffled = shuffled;
            self
        }

        pub fn finish(self) -> impl Strategy<Value = Vec<ops::Range<NonMaxU64>>> {
            proptest::collection::vec(
                (
                    // Size of the region (will be aligned)
                    self.size, // Gap after this region (will be aligned)
                    self.gap,
                ),
                self.num_ranges,
            )
            .prop_flat_map(move |size_gap_pairs| {
                // Calculate the maximum starting address that won't cause overflow
                let max_start = {
                    let total_space_needed: u64 =
                        size_gap_pairs.iter().map(|(size, gap)| size + gap).sum();

                    // Ensure we have headroom for alignment adjustments
                    u64::MAX.saturating_sub(total_space_needed)
                };

                (0..=max_start).prop_map(move |start_raw| {
                    let mut ranges = Vec::with_capacity(size_gap_pairs.len());
                    let mut current = start_raw;

                    for (size, gap) in &size_gap_pairs {
                        let start = NonMaxU64::new(current).unwrap();
                        let end = NonMaxU64::new(current + *size).unwrap();

                        ranges.push(start..end);

                        current += size + gap;
                    }

                    ranges
                })
            })
            .prop_perturb(move |mut ranges, mut rng| {
                if self.shuffled {
                    ranges.shuffle(&mut rng);
                }
                ranges
            })
        }
    }

    proptest! {
        #[test]
        fn insert_random(input in Ranges::new(1..750).finish()) {
            let mut input: Vec<_> = input.into_iter().enumerate().collect();

            let mut tree: RangeTree<NonMaxU64, usize, _> = RangeTree::try_new_in(Global).unwrap();

            for (idx, range) in input.iter() {
                tracing::debug!("inserting range {range:?}");
                tree.insert(range.clone(), *idx).unwrap();

                tree.assert_valid(true);
            }

            let ranges: Vec<_> = tree.ranges().collect();
            let values: Vec<_> = tree.values().copied().collect();

            input.sort_unstable_by(|(_, a), (_, b)| a.end.cmp(&b.end));
            assert_eq!(
                input
                    .iter()
                    .map(|(_, range)| range.clone())
                    .collect::<Vec<_>>(),
                ranges
            );
            assert_eq!(
                input.iter().map(|(idx, _)| *idx).collect::<Vec<_>>(),
                values
            );
        }

        #[test]
        fn insert_sorted(input in Ranges::new(1..750).shuffled(false).finish()) {
            let mut input: Vec<_> = input.into_iter().enumerate().collect();

            let mut tree: RangeTree<NonMaxU64, usize, _> = RangeTree::try_new_in(Global).unwrap();

            for (idx, range) in input.iter() {
                tracing::debug!("inserting range {range:?}");
                tree.insert(range.clone(), *idx).unwrap();

                tree.assert_valid(true);
            }

            let ranges: Vec<_> = tree.ranges().collect();
            let values: Vec<_> = tree.values().copied().collect();

            input.sort_unstable_by(|(_, a), (_, b)| a.end.cmp(&b.end));
            assert_eq!(
                input
                    .iter()
                    .map(|(_, range)| range.clone())
                    .collect::<Vec<_>>(),
                ranges
            );
            assert_eq!(
                input.iter().map(|(idx, _)| *idx).collect::<Vec<_>>(),
                values
            );
        }
    }

    #[test]
    fn smoke() {
        let input: Vec<_> = [100..200, 300..400, 500..600, 600..700]
            .into_iter()
            .map(|range| NonMaxU64::new(range.start).unwrap()..NonMaxU64::new(range.end).unwrap())
            .enumerate()
            .collect();

        let mut shuffled = input.clone();
        shuffled.shuffle(&mut rand::rng());

        let mut tree: RangeTree<NonMaxU64, usize, _> = RangeTree::try_new_in(Global).unwrap();

        for (idx, range) in shuffled {
            tracing::debug!("inserting range {range:?}");
            tree.insert(range.clone(), idx).unwrap();

            tree.assert_valid(true);
        }

        let ranges: Vec<_> = tree.ranges().collect();
        let values: Vec<_> = tree.values().copied().collect();

        assert_eq!(
            input
                .iter()
                .map(|(_, range)| range.clone())
                .collect::<Vec<_>>(),
            ranges
        );
        assert_eq!(
            input.iter().map(|(idx, _)| *idx).collect::<Vec<_>>(),
            values
        );
    }

    #[test]
    fn layout() {
        let (layout, children_offset) = const { internal_node_layout::<NonMaxU64>() };

        assert!(layout.align() >= align_of::<NodeRef>());
        assert!(
            layout.size()
                >= (size_of::<u64>() * NonMaxU64::B) + (size_of::<NodeRef>() * NonMaxU64::B)
        );
        assert_eq!(children_offset, size_of::<u64>() * NonMaxU64::B);

        let (layout, starts_offset, values_offset, next_leaf_offset) =
            const { leaf_node_layout::<NonMaxU64, usize>() };

        let size_pivots = size_of::<u64>() * NonMaxU64::B;
        let size_starts = size_of::<u64>() * NonMaxU64::B;
        let size_values = size_of::<usize>() * (NonMaxU64::B - 1);

        assert!(layout.align() >= align_of::<NodeRef>());
        assert!(
            layout.size() >= size_pivots + size_starts + size_values + (size_of::<NodeRef>()), // next leaf
            "leaf node layout too small! must be at least "
        );
        assert_eq!(starts_offset, size_pivots);
        assert_eq!(values_offset, size_pivots + size_starts);
        assert_eq!(next_leaf_offset, size_pivots + size_starts + size_values);
    }
}
