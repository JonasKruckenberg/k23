use core::num::NonZeroU64;

use range_tree::RangeTree;

use crate::common::nonzero;

mod common;

#[test]
fn lookup_hit() {
    tracing_subscriber::fmt::init();

    let mut tree: RangeTree<NonZeroU64, usize, _> = RangeTree::try_new().unwrap();

    tree.insert(nonzero!(100)..=nonzero!(200), 0).unwrap();

    assert_eq!(tree.get(nonzero!(100)), Some(&0));
    assert_eq!(tree.get(nonzero!(150)), Some(&0));
    assert_eq!(tree.get(nonzero!(200)), Some(&0));
    assert_eq!(tree.get(nonzero!(201)), None);
}
