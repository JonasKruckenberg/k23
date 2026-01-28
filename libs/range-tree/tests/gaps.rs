mod common;

// use nonmax::NonMaxU32;
// use range_tree::RangeTree;
//
// use crate::common::idx;
//
// #[test]
// fn empty_tree_gap() {
//     let tree: RangeTree<NonMaxU32, ()> = RangeTree::try_new().unwrap();
//     let gaps: Vec<_> = tree.gaps().collect();
//     // Empty tree should have no gaps (no keys to form gaps between)
//     // Actually, an empty tree has a "gap" representing the entire key space
//     // but since there are no keys, we don't yield anything
//     assert_eq!(gaps.len(), 1);
//     assert_eq!(gaps[0], NonMaxU32::ZERO..NonMaxU32::MAX);
// }
//
// #[test]
// fn single_element_gaps() {
//     let mut tree: RangeTree<NonMaxU32, &str> = RangeTree::try_new().unwrap();
//     tree.insert(idx!(NonMaxU32(100))..idx!(NonMaxU32(200)), "a")
//         .unwrap();
//
//     let gaps: Vec<_> = tree.gaps().collect();
//     // Should have gaps: [0, 100) and (200, MAX)
//     assert_eq!(gaps.len(), 2);
//     assert_eq!(gaps[0], NonMaxU32::ZERO..idx!(NonMaxU32(100)));
//     assert_eq!(gaps[1], idx!(NonMaxU32(200))..NonMaxU32::MAX);
// }
//
// #[test]
// fn consecutive_keys_no_internal_gaps() {
//     let mut tree: RangeTree<NonMaxU32, &str> = RangeTree::try_new().unwrap();
//     tree.insert(idx!(NonMaxU32(1))..idx!(NonMaxU32(2)), "a")
//         .unwrap();
//     tree.insert(idx!(NonMaxU32(2))..idx!(NonMaxU32(3)), "b")
//         .unwrap();
//     tree.insert(idx!(NonMaxU32(3))..idx!(NonMaxU32(4)), "c")
//         .unwrap();
//
//     let gaps: Vec<_> = tree.gaps().collect();
//     assert_eq!(gaps.len(), 2);
//     assert_eq!(gaps[0], NonMaxU32::ZERO..idx!(NonMaxU32(1)));
//     assert_eq!(gaps[1], idx!(NonMaxU32(4))..NonMaxU32::MAX);
// }
