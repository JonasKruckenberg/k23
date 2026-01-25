#![feature(allocator_api)]

mod common;

use std::alloc::Global;
use std::ops;

use nonmax::NonMaxU64;
use proptest::collection::SizeRange;
use proptest::prelude::*;
use rand::seq::SliceRandom;
use range_tree::RangeTree;

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
