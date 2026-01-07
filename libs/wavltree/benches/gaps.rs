#![feature(iter_map_windows)]

use std::alloc::Layout;
use std::hint::black_box;
use std::mem::offset_of;
use std::ops::Range;
use std::pin::Pin;
use std::ptr::NonNull;
use std::{cmp, mem};

use brie_tree::BTree;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use rand::prelude::SliceRandom;
use rand::{thread_rng, Rng};
use wavltree::{Linked, Links, WAVLTree};
use pin_project::pin_project;
use rand::distr::Uniform;

#[pin_project(!Unpin)]
#[derive(Debug, Default)]
struct WAVLEntry {
    range: Range<usize>,

    /// The address range covered by this region and its WAVL tree subtree, used when allocating new regions.
    subtree_range: Range<usize>,
    /// The largest gap in this subtree, used when allocating new regions.
    max_gap: usize,

    links: Links<Self>,
}
impl WAVLEntry {
    pub fn new(range: Range<usize>) -> Self {
        Self {
            subtree_range: range.clone(),
            max_gap: 0,
            range,
            links: Links::new(),
        }
    }

    /// Returns the left child node in the search tree of regions, used when allocating new regions.
    pub fn left_child(&self) -> Option<&Self> {
        // Safety: we have to trust the intrusive tree implementation here
        Some(unsafe { self.links.left()?.as_ref() })
    }

    /// Returns the right child node in the search tree of regions, used when allocating new regions.
    pub fn right_child(&self) -> Option<&Self> {
        // Safety: we have to trust the intrusive tree implementation here
        Some(unsafe { self.links.right()?.as_ref() })
    }

    /// Returns `true` if this nodes subtree contains a gap suitable for the given `layout`, used
    /// during gap-searching.
    pub fn suitable_gap_in_subtree(&self, layout: Layout) -> bool {
        // we need the layout to be padded to alignment
        debug_assert!(layout.size().is_multiple_of(layout.align()));

        self.max_gap >= layout.size()
    }
    /// Update the gap search metadata of this region. This method is called in the [`wavltree::Linked`]
    /// implementation below after each tree mutation that impacted this node or its subtree in some way
    /// (insertion, rotation, deletion).
    ///
    /// Returns `true` if this nodes metadata changed.
    fn update_gap_metadata(
        mut node: NonNull<Self>,
        left: Option<NonNull<Self>>,
        right: Option<NonNull<Self>>,
    ) -> bool {
        fn gap(left_last_byte: usize, right_first_byte: usize) -> usize {
            right_first_byte - left_last_byte
        }

        // Safety: we have to trust the intrusive tree implementation
        let node = unsafe { node.as_mut() };
        let mut left_max_gap = 0;
        let mut right_max_gap = 0;

        // recalculate the subtree_range start
        let old_subtree_range_start = if let Some(left) = left {
            // Safety: we have to trust the intrusive tree implementation
            let left = unsafe { left.as_ref() };
            let left_gap = gap(left.subtree_range.end, node.range.start);
            left_max_gap = cmp::max(left_gap, left.max_gap);
            mem::replace(&mut node.subtree_range.start, left.subtree_range.start)
        } else {
            mem::replace(&mut node.subtree_range.start, node.range.start)
        };

        // recalculate the subtree range end
        let old_subtree_range_end = if let Some(right) = right {
            // Safety: we have to trust the intrusive tree implementation
            let right = unsafe { right.as_ref() };
            let right_gap = gap(node.range.end, right.subtree_range.start);
            right_max_gap = cmp::max(right_gap, right.max_gap);
            mem::replace(&mut node.subtree_range.end, right.subtree_range.end)
        } else {
            mem::replace(&mut node.subtree_range.end, node.range.end)
        };

        // recalculate the map_gap
        let old_max_gap = mem::replace(&mut node.max_gap, cmp::max(left_max_gap, right_max_gap));

        old_max_gap != node.max_gap
            || old_subtree_range_start != node.subtree_range.start
            || old_subtree_range_end != node.subtree_range.end
    }

    // Propagate metadata updates to this regions parent in the search tree. If we had to update
    // our metadata the parent must update its metadata too.
    fn propagate_update_to_parent(mut maybe_node: Option<NonNull<Self>>) {
        while let Some(node) = maybe_node {
            // Safety: we have to trust the intrusive tree implementation
            let links = unsafe { &node.as_ref().links };
            let changed = Self::update_gap_metadata(node, links.left(), links.right());

            // if the metadata didn't actually change, we don't need to recalculate parents
            if !changed {
                return;
            }

            maybe_node = links.parent();
        }
    }
}

unsafe impl Linked for WAVLEntry {
    type Handle = Pin<Box<Self>>;
    type Key = usize;
    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        unsafe { NonNull::from(Box::leak(Pin::into_inner_unchecked(handle))) }
    }
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // Safety: `NonNull` *must* be constructed from a pinned reference
        // which the tree implementation upholds.
        Pin::new_unchecked(Box::from_raw(ptr.as_ptr()))
    }
    unsafe fn links(target: NonNull<Self>) -> NonNull<Links<WAVLEntry>> {
        target
            .map_addr(|addr| {
                let offset = offset_of!(Self, links);
                addr.checked_add(offset).unwrap()
            })
            .cast()
    }
    fn get_key(&self) -> &Self::Key {
        &self.range.start
    }
    fn after_insert(self: Pin<&mut Self>) {
        debug_assert_eq!(self.subtree_range.start, self.range.start);
        debug_assert_eq!(self.subtree_range.end, self.range.end);
        debug_assert_eq!(self.max_gap, 0);
        Self::propagate_update_to_parent(self.links.parent());
    }

    fn after_remove(self: Pin<&mut Self>, parent: Option<NonNull<Self>>) {
        Self::propagate_update_to_parent(parent);
    }

    fn after_rotate(
        self: Pin<&mut Self>,
        parent: NonNull<Self>,
        sibling: Option<NonNull<Self>>,
        lr_child: Option<NonNull<Self>>,
        side: wavltree::Side,
    ) {
        let this = self.project();
        // Safety: caller ensures ptr is valid
        let _parent = unsafe { parent.as_ref() };

        this.subtree_range.start = _parent.subtree_range.start;
        this.subtree_range.end = _parent.subtree_range.end;
        *this.max_gap = _parent.max_gap;

        if side == wavltree::Side::Left {
            Self::update_gap_metadata(parent, sibling, lr_child);
        } else {
            Self::update_gap_metadata(parent, lr_child, sibling);
        }
    }
}

pub struct GapsIter<'a> {
    layout: Layout,
    stack: Vec<&'a WAVLEntry>,
    prev_region_end: Option<usize>,
}

impl<'a> GapsIter<'a> {
    pub fn new(layout: Layout, root: &'a WAVLEntry) -> Self {
        let mut me = GapsIter {
            layout,
            stack: vec![],
            prev_region_end: None,
        };
        me.push_left_nodes(root);
        me
    }

    fn push_left_nodes(&mut self, mut node: &'a WAVLEntry) {
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
    type Item = Range<usize>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(node) = self.stack.pop() {
            if let Some(prev_region_end) = self.prev_region_end {
                // compute gap size
                let gap_size = node.range.start - prev_region_end;

                // if the gap is large enough yield it
                if gap_size >= self.layout.size() {
                    // no gap yielded for this node, continue traversal: push right subtree if interesting
                    if node.suitable_gap_in_subtree(self.layout)
                        && let Some(right) = node.right_child()
                    {
                        self.push_left_nodes(right);
                    }

                    let gap = prev_region_end..node.range.start;

                    // update prev_end to current node end before yielding
                    self.prev_region_end = Some(node.range.end);

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
            self.prev_region_end = Some(node.range.end);
        }

        None
    }
}

fn wavl(tree: &WAVLTree<WAVLEntry>, layout: Layout) {
    let gaps = GapsIter::new(layout, tree.root().get().unwrap());

    for gap in gaps {
        black_box(gap);
    }
}

fn brie(tree: &BTree<brie_tree::nonmax::NonMaxU64, (u64, u8)>, layout: Layout) {
    let gaps = tree
        .iter()
        .map_windows(|[(region_end, _), (_, (next_region_start, _))]| {
            region_end.get()..*next_region_start
        })
        .filter(|gap| (gap.end - gap.start) >= layout.size() as u64);

    for gap in gaps {
        black_box(gap);
    }
}

pub const KIB: usize = 1024;
pub const MIB: usize = KIB * 1024;
pub const GIB: usize = MIB * 1024;

fn bench_fibs(c: &mut Criterion) {
    let mut rng = thread_rng();

    let mut group = c.benchmark_group("Gap Search");
    for num_entries in (10..10_000).step_by(1000) {
        let mut entries = (0..num_entries * 2*MIB).step_by(2*MIB).collect::<Vec<_>>();
        entries.shuffle(&mut rng);

        let mut wavltree = WAVLTree::new();
        for i in &entries {
            wavltree.insert(Box::pin(WAVLEntry::new(*i..*i + rng.sample(Uniform::new(0, 2*MIB).unwrap()))));
        }

        let layout = Layout::from_size_align(MIB, 4096).unwrap();

        group.bench_with_input(
            BenchmarkId::new("WAVLTree", num_entries),
            &(wavltree, layout),
            |b, (wavltree, layout)| b.iter(|| wavl(wavltree, *layout)),
        );

        let mut brie_tree = BTree::new();
        for i in &entries {
            brie_tree.insert(
                brie_tree::nonmax::NonMaxU64::new(*i as u64).unwrap(),
                (*i as u64 + rng.sample(Uniform::new(0, 2*MIB).unwrap()) as u64, 0),
            );
        }

        group.bench_with_input(
            BenchmarkId::new("BrieTree", num_entries),
            &(brie_tree, layout),
            |b, (brie_tree, layout)| b.iter(|| brie(brie_tree, *layout)),
        );
    }
    group.finish();
}

criterion_group!(benches, bench_fibs);
criterion_main!(benches);
