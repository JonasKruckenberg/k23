#![feature(allocator_api)]

use std::alloc::Global;
use std::collections::BTreeMap;
use std::hint::black_box;
use std::mem::offset_of;
use std::ops::Range;
use std::pin::Pin;
use std::ptr::NonNull;

use brie_tree::BTree;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use nonmax::NonMaxU64;
use pin_project::pin_project;
use rand::Rng;
use rand::distr::Uniform;
use rand::prelude::SliceRandom;
use range_tree::RangeTree;
use wavltree::{Linked, Links, WAVLTree};

pub const KIB: u64 = 1024;
pub const MIB: u64 = KIB * 1024;

#[pin_project(!Unpin)]
#[derive(Debug, Default)]
struct WAVLEntry {
    range: Range<u64>,
    links: Links<Self>,
}
impl WAVLEntry {
    pub fn new(range: Range<u64>) -> Self {
        Self {
            range,
            links: Links::new(),
        }
    }
}

unsafe impl Linked for WAVLEntry {
    type Handle = Pin<Box<Self>>;
    type Key = u64;
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
        &self.range.end
    }
}

fn btreemap_insertions(insertions: &[Range<u64>]) {
    let mut map: BTreeMap<u64, (u64, u8)> = BTreeMap::new();

    for range in insertions {
        map.insert(range.end, (range.start, 0u8));
    }

    black_box(map);
}

fn brie_insertions(insertions: &[Range<u64>]) {
    let mut map: BTree<NonMaxU64, (NonMaxU64, u8)> = BTree::new();

    for range in insertions {
        let start = NonMaxU64::new(range.start).unwrap();
        let end = NonMaxU64::new(range.end).unwrap();

        map.insert(end, (start, 0u8));
    }

    black_box(map);
}

fn wavl_insertions(insertions: &[Range<u64>]) {
    let mut map: WAVLTree<WAVLEntry> = WAVLTree::new();

    for range in insertions {
        map.insert(Box::pin(WAVLEntry::new(range.clone())));
    }

    black_box(map);
}

fn range_insertions(insertions: &[Range<u64>]) {
    let mut map: RangeTree<NonMaxU64, u8, _> = RangeTree::try_new_in(Global).unwrap();

    for range in insertions {
        let range = NonMaxU64::new(range.start).unwrap()..NonMaxU64::new(range.end).unwrap();

        map.insert(range, 0u8).unwrap();
    }

    black_box(map);
}

fn bench_insertions(c: &mut Criterion) {
    let mut rng = rand::rng();

    let mut group = c.benchmark_group("Insertions");
    for num_entries in (10..10_000).step_by(1000) {
        let mut ranges = (0..num_entries * 2 * MIB)
            .step_by(2 * MIB as usize)
            .map(|base| base..base + rng.sample(Uniform::new(0, 2 * MIB).unwrap()))
            .collect::<Vec<_>>();

        ranges.shuffle(&mut rng);

        group.bench_with_input(
            BenchmarkId::new("BTreeMap", num_entries),
            ranges.as_slice(),
            |b, ranges| b.iter(|| btreemap_insertions(ranges)),
        );

        group.bench_with_input(
            BenchmarkId::new("BrieTree", num_entries),
            ranges.as_slice(),
            |b, ranges| b.iter(|| brie_insertions(ranges)),
        );

        group.bench_with_input(
            BenchmarkId::new("WAVLTree", num_entries),
            ranges.as_slice(),
            |b, ranges| b.iter(|| wavl_insertions(ranges)),
        );

        group.bench_with_input(
            BenchmarkId::new("RangeTree", num_entries),
            ranges.as_slice(),
            |b, ranges| b.iter(|| range_insertions(ranges)),
        );
    }
    group.finish();
}

// fn btreemap_lookups(map: &BTreeMap<u64, (u64, u8)>, lookups: &[u64]) {
//     for lookup in lookups {
//         let (_end, (start, _flags)) = map.range(lookup..).next().unwrap();
//         let offset = lookup.checked_sub(*start).unwrap();
//         black_box(offset);
//     }
// }
//
// fn brie_lookups(map: &BTree<NonMaxU64, (NonMaxU64, u8)>, lookups: &[u64]) {
//     for lookup in lookups {
//         let lookup = NonMaxU64::new(*lookup).unwrap();
//         let (_end, (start, _flags)) = map.range(lookup..).next().unwrap();
//         let offset = lookup.get().checked_sub(start.get()).unwrap();
//         black_box(offset);
//     }
// }
//
// fn wavl_lookups(map: &WAVLTree<WAVLEntry>, lookups: &[u64]) {
//     for lookup in lookups {
//         let entry = map.range(*lookup..).next().unwrap();
//         let offset = lookup
//             .checked_sub(entry.range.start)
//             .unwrap_or_else(|| panic!("expected {lookup} to be within {entry:?}"));
//         black_box(offset);
//     }
// }
//
// fn range_lookups(map: &RangeTree<NonMaxU64, u8>, lookups: &[u64]) {
//     for lookup in lookups {
//         let lookup = NonMaxU64::new(*lookup).unwrap();
//         black_box(map.get_containing(lookup).unwrap());
//     }
// }

// fn bench_lookups_hits(c: &mut Criterion) {
//     let mut rng = rand::rng();
//
//     let mut group = c.benchmark_group("Lookups Hits");
//     for num_entries in (10..10_000).step_by(1000) {
//         let mut ranges = (0..num_entries * 2 * MIB)
//             .step_by(2 * MIB as usize)
//             .map(|base| base..base + rng.sample(Uniform::new(0, 2 * MIB).unwrap()))
//             .collect::<Vec<_>>();
//
//         ranges.shuffle(&mut rng);
//
//         let mut lookups = vec![];
//         for range in &ranges {
//             for _ in 0..1_000 {
//                 let lookup = rng.sample(Uniform::new(range.start, range.end).unwrap());
//                 lookups.push(lookup);
//             }
//         }
//
//         {
//             let mut map: BTreeMap<u64, (u64, u8)> = BTreeMap::new();
//
//             for range in &ranges {
//                 map.insert(range.end, (range.start, 0u8));
//             }
//
//             group.bench_with_input(
//                 BenchmarkId::new("BTreeMap", num_entries),
//                 &(map, lookups.as_slice()),
//                 |b, (map, lookups)| b.iter(|| btreemap_lookups(map, lookups)),
//             );
//         }
//
//         {
//             let mut map: BTree<NonMaxU64, (NonMaxU64, u8)> = BTree::new();
//
//             for range in &ranges {
//                 let start = NonMaxU64::new(range.start).unwrap();
//                 let end = NonMaxU64::new(range.end).unwrap();
//
//                 map.insert(end, (start, 0u8));
//             }
//
//             group.bench_with_input(
//                 BenchmarkId::new("BrieTree", num_entries),
//                 &(map, lookups.as_slice()),
//                 |b, (map, lookups)| b.iter(|| brie_lookups(map, lookups)),
//             );
//         }
//
//         {
//             let mut map: WAVLTree<WAVLEntry> = WAVLTree::new();
//
//             for range in &ranges {
//                 map.insert(Box::pin(WAVLEntry::new(range.clone())));
//             }
//
//             group.bench_with_input(
//                 BenchmarkId::new("WAVLTree", num_entries),
//                 &(map, lookups.as_slice()),
//                 |b, (map, lookups)| b.iter(|| wavl_lookups(map, lookups)),
//             );
//         }
//
//         {
//             let mut map: RangeTree<NonMaxU64, u8, _> = RangeTree::try_new().unwrap();
//
//             for range in &ranges {
//                 let start = NonMaxU64::new(range.start).unwrap();
//                 let end = NonMaxU64::new(range.end).unwrap();
//
//                 map.insert(start..end, 0u8).unwrap();
//             }
//
//             group.bench_with_input(
//                 BenchmarkId::new("Stupid Simple", num_entries),
//                 &(map, lookups.as_slice()),
//                 |b, (map, lookups)| b.iter(|| range_lookups(map, lookups)),
//             );
//         }
//     }
//     group.finish();
// }

// fn bench_lookups_misses(c: &mut Criterion) {
//     let mut rng = rand::rng();
//
//     let mut group = c.benchmark_group("Lookups Misses");
//     for num_entries in (10..10_000).step_by(1000) {
//         let mut ranges = (0..num_entries * 2 * MIB)
//             .step_by(2 * MIB as usize)
//             .map(|base| base..base + rng.sample(Uniform::new(0, 2 * MIB).unwrap()))
//             .collect::<Vec<_>>();
//
//         ranges.shuffle(&mut rng);
//
//         let gaps = ranges
//             .windows(2)
//             .map(|window| match window {
//                 [range, next] => range.end..next.start,
//                 _ => unreachable!(),
//             })
//             .filter(|range| !range.is_empty());
//
//         let mut lookups = vec![];
//         for gap in gaps {
//             for _ in 0..1_000 {
//                 let lookup = rng.sample(Uniform::new(gap.start, gap.end).unwrap());
//                 lookups.push(lookup);
//             }
//         }
//
//         {
//             let mut map: BTreeMap<u64, (u64, u8)> = BTreeMap::new();
//
//             for range in &ranges {
//                 map.insert(range.end, (range.start, 0u8));
//             }
//
//             group.bench_with_input(
//                 BenchmarkId::new("BTreeMap", num_entries),
//                 &(map, lookups.as_slice()),
//                 |b, (map, lookups)| b.iter(|| btreemap_lookups(map, lookups)),
//             );
//         }
//
//         {
//             let mut map: BTree<NonMaxU64, (NonMaxU64, u8)> = BTree::new();
//
//             for range in &ranges {
//                 let start = NonMaxU64::new(range.start).unwrap();
//                 let end = NonMaxU64::new(range.end).unwrap();
//
//                 map.insert(end, (start, 0u8));
//             }
//
//             group.bench_with_input(
//                 BenchmarkId::new("BrieTree", num_entries),
//                 &(map, lookups.as_slice()),
//                 |b, (map, lookups)| b.iter(|| brie_lookups(map, lookups)),
//             );
//         }
//
//         {
//             let mut map: WAVLTree<WAVLEntry> = WAVLTree::new();
//
//             for range in &ranges {
//                 map.insert(Box::pin(WAVLEntry::new(range.clone())));
//             }
//
//             group.bench_with_input(
//                 BenchmarkId::new("WAVLTree", num_entries),
//                 &(map, lookups.as_slice()),
//                 |b, (map, lookups)| b.iter(|| wavl_lookups(map, lookups)),
//             );
//         }
//
//         {
//             let mut map: RangeTree<NonMaxU64, u8> = RangeTree::try_new().unwrap();
//
//             for range in &ranges {
//                 let start = NonMaxU64::new(range.start).unwrap();
//                 let end = NonMaxU64::new(range.end).unwrap();
//
//                 map.insert(start..end, 0u8).unwrap();
//             }
//
//             group.bench_with_input(
//                 BenchmarkId::new("Stupid Simple", num_entries),
//                 &(map, lookups.as_slice()),
//                 |b, (map, lookups)| b.iter(|| stupid_lookups(map, lookups)),
//             );
//         }
//     }
//     group.finish();
// }

criterion_group!(
    benches,
    bench_insertions,
    // bench_lookups_hits,
    // bench_lookups_misses
);
criterion_main!(benches);
