use nonmax::NonMaxU64;
use range_tree::RangeTree;

use crate::common::idx;

mod common;

#[test]
fn lookup_hit() {
    tracing_subscriber::fmt::init();

    let mut tree: RangeTree<NonMaxU64, usize, _> = RangeTree::try_new().unwrap();

    tree.insert(idx!(NonMaxU64(0))..idx!(NonMaxU64(100)), 0)
        .unwrap();

    assert_eq!(tree.get(idx!(NonMaxU64(0))), Some(&0));
    assert_eq!(tree.get(idx!(NonMaxU64(50))), Some(&0));
    assert_eq!(tree.get(idx!(NonMaxU64(100))), None);
}
