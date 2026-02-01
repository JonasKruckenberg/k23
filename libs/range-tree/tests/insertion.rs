#![feature(allocator_api)]
#![feature(new_range_api)]

mod common;

use core::num::NonZeroU64;
use std::alloc::Global;
use std::range::RangeInclusive;

use rand::seq::SliceRandom;
use range_tree::{OverlapError, RangeTree};

use crate::common::nonzero;

#[test]
fn smoke() {
    let input: Vec<_> = [
        RangeInclusive {
            start: nonzero!(100),
            end: nonzero!(200),
        },
        RangeInclusive {
            start: nonzero!(300),
            end: nonzero!(400),
        },
        RangeInclusive {
            start: nonzero!(500),
            end: nonzero!(600),
        },
        RangeInclusive {
            start: nonzero!(600),
            end: nonzero!(700),
        },
    ]
    .into_iter()
    .enumerate()
    .collect();

    let mut shuffled = input.clone();
    shuffled.shuffle(&mut rand::rng());

    let mut tree: RangeTree<NonZeroU64, usize, _> = RangeTree::try_new_in(Global).unwrap();

    for (idx, range) in shuffled {
        tracing::debug!("inserting range {range:?}");
        tree.insert(range.clone(), idx).unwrap();

        tree.assert_valid();
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
fn overlap() {
    tracing_subscriber::fmt::init();

    let mut tree: RangeTree<NonZeroU64, usize, _> = RangeTree::try_new_in(Global).unwrap();

    tree.insert(nonzero!(100)..=nonzero!(200), 0).unwrap();
    tree.insert(nonzero!(300)..=nonzero!(400), 1).unwrap();

    assert!(matches!(
        tree.insert(nonzero!(100)..=nonzero!(110), 2),
        Err(OverlapError)
    ));
    assert!(matches!(
        tree.insert(nonzero!(199)..=nonzero!(201), 2),
        Err(OverlapError)
    ));
    assert!(matches!(
        tree.insert(nonzero!(110)..=nonzero!(190), 2),
        Err(OverlapError)
    ));
    assert!(matches!(
        tree.insert(nonzero!(110)..=nonzero!(301), 1),
        Err(OverlapError)
    ));
}
