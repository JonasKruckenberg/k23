#![cfg_attr(not(test), no_std)]
#![feature(allocator_api)]
extern crate alloc;

mod cursor;
mod idx;
mod iter;
mod node;
mod simd;
mod stack;

use core::alloc::{AllocError, Allocator};
use core::ops;

pub use cursor::{Cursor, CursorMut};
use idx::Idx;

use crate::node::{NodePool, NodePos, NodeRef, UninitNodeRef, marker};
use crate::stack::Height;

pub struct RangeTree<I: Idx, V, A: Allocator> {
    internal: NodePool<I, marker::Internal<V>>,
    leaf: NodePool<I, marker::Leaf<V>>,
    root: NodeRef<marker::LeafOrInternal<V>>,
    height: Height<I>,
    allocator: A,
}

impl<I: Idx, V, A: Allocator> RangeTree<I, V, A> {
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
    fn init_root(&mut self, root: UninitNodeRef<marker::Leaf<V>>) {
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
        cursor.seek(range.end.to_raw());
        cursor.insert_before(range, value)
    }

    pub fn assert_valid(&self, assert_sorted: bool) {
        let mut last_leaf = None;
        self.check_node(
            self.root,
            self.height,
            assert_sorted,
            None,
            I::MAX,
            &mut last_leaf,
        );

        // Ensure the linked list of leaf nodes is properly terminated.
        assert_eq!(unsafe { last_leaf.unwrap().next_leaf(&self.leaf) }, None);
    }

    fn check_node(
        &self,
        node: NodeRef<marker::LeafOrInternal<V>>,
        height: Height<I>,
        assert_sorted: bool,
        min: Option<I::Raw>,
        max: I::Raw,
        prev_leaf: &mut Option<NodeRef<marker::Leaf<V>>>,
    ) {
        let Some(down) = height.down() else {
            self.check_leaf_node(unsafe { node.cast() }, assert_sorted, min, max, prev_leaf);
            return;
        };

        let node = unsafe { node.cast() };

        let keys =
            || (0..I::B).map(|i| unsafe { node.pivot(NodePos::new_unchecked(i), &self.internal) });

        // The last 2 keys must be MAX.
        assert_eq!(keys().nth(I::B - 1).unwrap(), I::MAX);
        assert_eq!(keys().nth(I::B - 2).unwrap(), I::MAX);

        // All MAX keys must be after non-MAX keys,
        assert!(keys().is_sorted_by_key(|key| key == I::MAX));

        // Keys must be sorted in increasing order.
        if assert_sorted {
            assert!(keys().is_sorted_by(|&a, &b| I::cmp(a, b).is_le()));
            if let Some(min) = min {
                assert!(keys().all(|key| I::cmp(key, min).is_ge()));
            }
            assert!(keys().all(|key| key == I::MAX || I::cmp(key, max).is_le()));
        }

        let len = keys().take_while(|&key| key != I::MAX).count() + 1;
        let is_root = height == self.height;

        // Non-root nodes must be at least half full. Non-leaf root nodes must
        // have at least 2 elements.
        if is_root {
            assert!(len >= 2);
        } else {
            assert!(len >= I::B / 2);
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
                    if key == I::MAX { max } else { key },
                    prev_leaf,
                );
                prev_key = Some(key);
            }
        }
    }

    fn check_leaf_node(
        &self,
        node: NodeRef<marker::Leaf<V>>,
        assert_sorted: bool,
        min: Option<I::Raw>,
        max: I::Raw,
        prev_leaf: &mut Option<NodeRef<marker::Leaf<V>>>,
    ) {
        let keys =
            || (0..I::B).map(|i| unsafe { node.pivot(NodePos::new_unchecked(i), &self.leaf) });

        // The last key must be MAX.
        assert_eq!(keys().nth(I::B - 1).unwrap(), I::MAX);

        // All MAX keys must be after non-MAX keys,
        assert!(keys().is_sorted_by_key(|key| key == I::MAX));

        // Keys must be sorted in increasing order.
        if assert_sorted {
            assert!(keys().is_sorted_by(|&a, &b| I::cmp(a, b).is_le()));
            if let Some(min) = min {
                assert!(keys().all(|key| I::cmp(key, min).is_ge()));
            }
            assert!(keys().all(|key| key == I::MAX || I::cmp(key, max).is_le()));
        }

        let len = keys().take_while(|&key| key != I::MAX).count();
        let is_root = self.height == Height::LEAF;

        // Non-root nodes must be at least half full.
        if !is_root {
            assert!(len >= I::B / 2);
        }

        // The last key must be equal to the maximum for this sub-tree.
        if max != I::MAX {
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
        fn insert_random(mut input in Ranges::new(1..1000).finish()) {
            let mut tree: RangeTree<NonMaxU64, usize, _> =
                RangeTree::try_new_in(Global).unwrap();

            for (idx, range) in input.iter().enumerate() {
                println!("inserting range {range:?}");
                tree.insert(range.clone(), idx).unwrap();

                tree.assert_valid(true);
            }

            let ranges: Vec<_> = tree.ranges().collect();
            let values: Vec<_> = tree.values().copied().collect();

            input.sort_unstable_by(|a, b| a.end.cmp(&b.end));
            assert_eq!(input, ranges);
            // assert_eq!(input, values);
        }

        #[test]
        fn insert_sorted(mut input in Ranges::new(1..1000).shuffled(false).finish()) {
            let mut tree: RangeTree<NonMaxU64, usize, _> =
                RangeTree::try_new_in(Global).unwrap();

            for (idx, range) in input.iter().enumerate() {
                println!("inserting range {range:?}");
                tree.insert(range.clone(), idx).unwrap();

                tree.assert_valid(true);
            }

            let ranges: Vec<_> = tree.ranges().collect();

            input.sort_unstable_by(|a, b| a.end.cmp(&b.end));
            assert_eq!(input, ranges);
        }
    }

    #[test]
    fn smoke() {
        let mut input: Vec<_> = [
            9048959..9050743,
            7376378..7378870,
            7029025..7030107,
            3296991..3298651,
            9017035..9020669,
            3811427..3813899,
            1298150..1298449,
            8545363..8546770,
            4601879..4605500,
            124870..128878,
            3225715..3229378,
            9066040..9070057,
            8946829..8950481,
            1980642..1981410,
            9082338..9084591,
            9126468..9129798,
            // 5408990..5409042,
            // 4460276..4461031,
            // 7716648..7718835,
            // 7557250..7559903,
            // 7240558..7241800,
            // 3693758..3696905,
            // 426826..430407,
            // 3422322..3423416,
            // 4098760..4099199,
            // 6353542..6357079,
            // 7494944..7496189,
            // 2567118..2569888,
            // 1125162..1126174,
            // 7450205..7453367,
        ].into_iter().map(|range| NonMaxU64::new(range.start).unwrap()..NonMaxU64::new(range.end).unwrap()).collect();

        let mut tree: RangeTree<NonMaxU64, usize, _> = RangeTree::try_new_in(Global).unwrap();

        for (idx, range) in input.iter().enumerate() {
            println!("inserting range {range:?}");
            tree.insert(range.clone(), idx).unwrap();

            tree.assert_valid(true);
        }

        let ranges: Vec<_> = tree.ranges().collect();
        let values: Vec<_> = tree.values().copied().collect();

        input.sort_unstable_by(|a, b| a.end.cmp(&b.end));
        assert_eq!(input.as_slice(), ranges.as_slice());
        // assert_eq!(input, values);
    }
}
