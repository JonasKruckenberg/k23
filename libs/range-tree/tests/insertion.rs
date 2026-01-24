#![feature(allocator_api)]

use std::alloc::Global;

use nonmax::NonMaxU64;
use rand::seq::SliceRandom;
use range_tree::{InsertError, RangeTree};

macro_rules! idx {
    ($raw:literal) => {{ const { NonMaxU64::new($raw).unwrap() } }};
}

#[test]
fn smoke() {
    let input: Vec<_> = [
        idx!(100)..idx!(200),
        idx!(300)..idx!(400),
        idx!(500)..idx!(600),
        idx!(600)..idx!(700),
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

    tree.insert(idx!(0)..idx!(100), 0).unwrap();
    tree.insert(idx!(200)..idx!(300), 1).unwrap();

    assert!(matches!(
        tree.insert(idx!(0)..idx!(10), 2),
        Err(InsertError::Overlap)
    ));
    assert!(matches!(
        tree.insert(idx!(99)..idx!(101), 2),
        Err(InsertError::Overlap)
    ));
    assert!(matches!(
        tree.insert(idx!(10)..idx!(90), 2),
        Err(InsertError::Overlap)
    ));
    assert!(matches!(
        tree.insert(idx!(10)..idx!(201), 1),
        Err(InsertError::Overlap)
    ));
}
