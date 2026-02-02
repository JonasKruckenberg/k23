#![feature(new_range_api)]

mod common;

use std::num::NonZeroU32;
use std::ops::Bound;

use range_tree::RangeTree;

use crate::common::nonzero;

#[test]
fn empty_tree_gap() {
    let tree: RangeTree<NonZeroU32, ()> = RangeTree::try_new().unwrap();
    let gaps: Vec<_> = tree.gaps().collect();
    println!("{gaps:?}");
    // Empty tree should have no gaps (no keys to form gaps between)
    // Actually, an empty tree has a "gap" representing the entire key space
    // but since there are no keys, we don't yield anything
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0], (Bound::Unbounded, Bound::Unbounded));
}

#[test]
fn single_element_gaps() {
    let mut tree: RangeTree<NonZeroU32, &str> = RangeTree::try_new().unwrap();
    tree.insert(nonzero!(100)..=nonzero!(199), "a").unwrap();

    let gaps: Vec<_> = tree.gaps().collect();
    println!("{gaps:?}");
    // Should have gaps: [MIN, 100) and (200, MAX)
    assert_eq!(gaps.len(), 2);
    assert_eq!(gaps[0], (Bound::Unbounded, Bound::Excluded(nonzero!(100))));
    assert_eq!(gaps[1], (Bound::Included(nonzero!(200)), Bound::Unbounded));
}

#[test]
fn consecutive_keys_no_internal_gaps() {
    let mut tree: RangeTree<NonZeroU32, &str> = RangeTree::try_new().unwrap();
    tree.insert(nonzero!(1)..=nonzero!(2), "a").unwrap();
    tree.insert(nonzero!(3)..=nonzero!(4), "b").unwrap();
    tree.insert(nonzero!(5)..=nonzero!(6), "c").unwrap();

    let gaps: Vec<_> = tree.gaps().collect();
    println!("{gaps:?}");
    assert_eq!(gaps.len(), 2);
    assert_eq!(gaps[0], (Bound::Unbounded, Bound::Excluded(nonzero!(1))));
    assert_eq!(gaps[1], (Bound::Included(nonzero!(7)), Bound::Unbounded));
}
