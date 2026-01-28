#![feature(allocator_api)]

mod common;

use core::num::NonZeroU64;
use std::alloc::Global;

use rand::seq::SliceRandom;
use range_tree::{InsertError, RangeTree};

use crate::common::nonzero;

#[test]
fn smoke() {
    let input: Vec<_> = [
        nonzero!(100)..nonzero!(200),
        nonzero!(300)..nonzero!(400),
        nonzero!(500)..nonzero!(600),
        nonzero!(600)..nonzero!(700),
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
fn overlap() {
    tracing_subscriber::fmt::init();

    let mut tree: RangeTree<NonZeroU64, usize, _> = RangeTree::try_new_in(Global).unwrap();

    tree.insert(nonzero!(100)..nonzero!(200), 0).unwrap();
    tree.insert(nonzero!(300)..nonzero!(400), 1).unwrap();

    assert!(matches!(
        tree.insert(nonzero!(100)..nonzero!(110), 2),
        Err(InsertError::Overlap)
    ));
    assert!(matches!(
        tree.insert(nonzero!(199)..nonzero!(201), 2),
        Err(InsertError::Overlap)
    ));
    assert!(matches!(
        tree.insert(nonzero!(110)..nonzero!(190), 2),
        Err(InsertError::Overlap)
    ));
    assert!(matches!(
        tree.insert(nonzero!(110)..nonzero!(301), 1),
        Err(InsertError::Overlap)
    ));
}
