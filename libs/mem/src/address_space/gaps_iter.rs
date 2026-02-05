use alloc::vec;
use alloc::vec::Vec;
use core::alloc::Layout;
use core::ops::Range;

use kmem::VirtualAddress;

use crate::address_space::region::AddressSpaceRegion;

#[derive(Debug, Clone)]
pub struct GapsIter<'a> {
    layout: Layout,
    stack: Vec<&'a AddressSpaceRegion>,
    prev_region_end: Option<VirtualAddress>,
}

impl<'a> GapsIter<'a> {
    pub fn new(layout: Layout, root: &'a AddressSpaceRegion) -> Self {
        let mut me = GapsIter {
            layout,
            stack: vec![],
            prev_region_end: None,
        };
        me.push_left_nodes(root);
        me
    }

    fn push_left_nodes(&mut self, mut node: &'a AddressSpaceRegion) {
        loop {
            self.stack.push(node);
            if node.suitable_gap_in_subtree(self.layout)
                && let Some(left) = node.left_child()
            {
                node = left;
            } else {
                break;
            }
        }
    }
}

impl Iterator for GapsIter<'_> {
    type Item = Range<VirtualAddress>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(node) = self.stack.pop() {
            if let Some(prev_region_end) = self.prev_region_end {
                // compute gap size
                let gap_size = node.range().start.offset_from_unsigned(prev_region_end);

                // if the gap is large enough yield it
                if gap_size >= self.layout.size() {
                    // no gap yielded for this node, continue traversal: push right subtree if interesting
                    if node.suitable_gap_in_subtree(self.layout)
                        && let Some(right) = node.right_child()
                    {
                        self.push_left_nodes(right);
                    }

                    let gap = prev_region_end..node.range().start;

                    // update prev_end to current node end before yielding
                    self.prev_region_end = Some(node.range().end);

                    return Some(gap);
                }
            }

            // no gap yielded for this node, continue traversal: push right subtree if interesting
            if node.suitable_gap_in_subtree(self.layout)
                && let Some(right) = node.right_child()
            {
                self.push_left_nodes(right);
            }

            // ensure prev_end reflects the most-recent visited node end
            self.prev_region_end = Some(node.range().end);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use core::alloc::Layout;
    use core::iter;

    use kmem::test_utils::proptest::regions_virt;
    use kmem::{AddressRangeExt, Arch, GIB, MemoryAttributes, VirtualAddress, for_every_arch};
    use proptest::prelude::*;
    use wavltree::WAVLTree;

    use super::*;
    use crate::address_space::region::AddressSpaceRegion;

    for_every_arch!(A => {
        proptest! {
            #[test]
            fn iterate_gaps(regions in regions_virt(1..50, A::GRANULE_SIZE, 1*GIB, 1*GIB)) {
                let mut tree: WAVLTree<AddressSpaceRegion> = WAVLTree::new();

                for region in &regions {
                    tree.insert(Box::pin(AddressSpaceRegion::new(
                        region.start,
                        MemoryAttributes::new().with(MemoryAttributes::READ, true),
                        Layout::from_size_align(region.len(), A::GRANULE_SIZE).unwrap(),
                    )));
                }

                // let gap_before = VirtualAddress::MIN..regions[0].start;
                // let gap_after = regions.last().unwrap().end..VirtualAddress::MAX;
                let gaps_between = regions.windows(2).map(|regions| match regions {
                    [region, next_region, ..] => region.end..next_region.start,
                    _ => unreachable!(),
                });

                let expected_gaps: Vec<_> = gaps_between.collect();

                let gaps: Vec<_> = GapsIter::new(
                    Layout::from_size_align(A::GRANULE_SIZE, A::GRANULE_SIZE).unwrap(),
                    tree.root().get().unwrap(),
                )
                .collect();

                prop_assert_eq!(expected_gaps, gaps);
            }
        }
    });
}
