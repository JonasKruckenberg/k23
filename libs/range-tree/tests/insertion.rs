#![feature(allocator_api)]

mod common;

use std::alloc::Global;

use nonmax::NonMaxU64;
use rand::seq::SliceRandom;
use range_tree::RangeTree;

use crate::common::idx;

#[test]
fn smoke() {
    let input: Vec<_> = [
        idx!(NonMaxU64(100))..idx!(NonMaxU64(200)),
        idx!(NonMaxU64(300))..idx!(NonMaxU64(400)),
        idx!(NonMaxU64(500))..idx!(NonMaxU64(600)),
        idx!(NonMaxU64(600))..idx!(NonMaxU64(700)),
    ]
    .into_iter()
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
fn overlap() {
    tracing_subscriber::fmt::init();

    let mut tree: RangeTree<NonMaxU64, usize, _> = RangeTree::try_new_in(Global).unwrap();

    tree.insert(idx!(NonMaxU64(0))..idx!(NonMaxU64(100)), 0).unwrap();
    tree.insert(idx!(NonMaxU64(200))..idx!(NonMaxU64(300)), 1).unwrap();

    assert!(matches!(
        tree.insert(idx!(NonMaxU64(0))..idx!(NonMaxU64(10)), 2),
        Err(InsertError::Overlap)
    ));
    assert!(matches!(
        tree.insert(idx!(NonMaxU64(99))..idx!(NonMaxU64(101)), 2),
        Err(InsertError::Overlap)
    ));
    assert!(matches!(
        tree.insert(idx!(NonMaxU64(10))..idx!(NonMaxU64(90)), 2),
        Err(InsertError::Overlap)
    ));
    assert!(matches!(
        tree.insert(idx!(NonMaxU64(10))..idx!(NonMaxU64(201)), 1),
        Err(InsertError::Overlap)
    ));
}
