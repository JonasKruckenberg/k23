#![feature(allocator_api)]
#![feature(new_range_api)]
#![feature(range_bounds_is_empty)]
#![no_main]

use std::alloc::Global;
use std::fmt::Debug;
use std::num::{NonZeroU8, NonZeroU16, NonZeroU32, NonZeroU64, NonZeroU128};
use std::ops::{Bound, RangeBounds};
use std::range::RangeInclusive;

use libfuzzer_sys::arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use range_tree::OverlapError;
use range_tree::{RangeTree, RangeTreeIndex};

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
struct Index<Int>(Int);

trait Idx: Copy {
    fn checked_increment(self) -> Option<Self>;
}

macro_rules! impl_index {
    ($($int:ident $nonzero:ident),*) => {
        $(
            impl Arbitrary<'_> for Index<$nonzero> {
                fn arbitrary(u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<Self> {
                    Ok(Index(
                        $nonzero::new(u.int_in_range(1..=$int::MAX)?).unwrap()
                    ))
                }
            }

            impl RangeTreeIndex for Index<$nonzero> {
                type Int = $nonzero;

                fn from_int(int: Self::Int) -> Self {
                    Self(int)
                }

                fn to_int(self) -> Self::Int {
                    self.0
                }
            }

            impl Idx for Index<$nonzero> {
                fn checked_increment(self) -> Option<Self> {
                    self.0.checked_add(1).map(Self)
                }
            }
        )*
    };
}

impl_index! {
    u8 NonZeroU8,
    u16 NonZeroU16,
    u32 NonZeroU32,
    u64 NonZeroU64,
    u128 NonZeroU128
}

#[derive(Debug, Arbitrary)]
enum Action<Index, Value> {
    Clear,
    Insert {
        start: Index,
        end: Index,
        value: Value,
    },
    Get(Index),
    Remove(Index),
    Range(Bound<Index>, Bound<Index>),
    Iter(Option<Bound<Index>>),
    Gaps,
    Cursor(Option<Bound<Index>>, Vec<CursorAction>),
    CursorMut(Option<Bound<Index>>, Vec<CursorMutAction<Index, Value>>),
}

#[derive(Arbitrary, Debug)]
enum CursorAction {
    Next,
    Prev,
}

#[derive(Arbitrary, Debug)]
enum CursorMutAction<Index, Value> {
    Next,
    Prev,
    Insert {
        start: Index,
        end: Index,
        value: Value,
    },
    Replace(Value),
    Remove,
}

#[derive(Arbitrary, Debug)]
enum IndexType<Value> {
    U8(Vec<Action<Index<NonZeroU8>, Value>>),
    U16(Vec<Action<Index<NonZeroU16>, Value>>),
    U32(Vec<Action<Index<NonZeroU32>, Value>>),
    U64(Vec<Action<Index<NonZeroU64>, Value>>),
    U128(Vec<Action<Index<NonZeroU128>, Value>>),
}

#[derive(Arbitrary, Debug)]
enum ValueType {
    Empty(IndexType<()>),
    U8(IndexType<u8>),
    U16(IndexType<u16>),
    U32(IndexType<u32>),
    U64(IndexType<u64>),
    U128(IndexType<u128>),
}

fn run<
    'a,
    Index: Ord + RangeTreeIndex + Arbitrary<'a> + Debug + Idx,
    Value: Eq + Arbitrary<'a> + Debug + Copy,
>(
    actions: Vec<Action<Index, Value>>,
) {
    let mut tree = RangeTree::try_new_in(Global).unwrap();
    let mut vec: Vec<(RangeInclusive<Index>, Value)> = vec![];

    for action in actions {
        match action {
            Action::Clear => {
                tree.clear();
                vec.clear();
            }
            Action::Insert { start, end, value } => {
                let res = tree.insert(RangeInclusive { start, end }, value);

                let index = vec.partition_point(|(range, _v)| range.end < end);

                if index != vec.len() && vec[index].0.start < end {
                    assert_eq!(res, Err(OverlapError));
                } else if index != 0 && vec[index - 1].0.end > start {
                    assert_eq!(res, Err(OverlapError));
                } else {
                    vec.insert(index, (RangeInclusive { start, end }, value));
                    assert_eq!(res, Ok(()));
                }
            }
            Action::Get(search) => {
                let value = tree.get(search);
                let index = vec.partition_point(|(range, _v)| range.end < search);

                if index != vec.len() && vec[index].0.start <= search {
                    assert_eq!(value, Some(&vec[index].1));
                } else {
                    assert_eq!(value, None);
                }
            }
            Action::Remove(search) => {
                let value = tree.remove(search);
                let index = vec.partition_point(|(range, _v)| range.end < search);

                if index != vec.len() && vec[index].0.start <= search {
                    assert_eq!(value, Some(vec[index].1));
                    vec.remove(index);
                } else {
                    assert_eq!(value, None);
                }
            }
            Action::Range(start, end) => {
                let range = tree.range((start, end));
                let entries: Vec<_> = range.map(|(k, &v)| (k, v)).collect();
                let start = match start {
                    Bound::Unbounded => 0,
                    Bound::Included(start) => vec.partition_point(|(range, _v)| range.end < start),
                    Bound::Excluded(start) => vec.partition_point(|(range, _v)| range.end <= start),
                };
                let end = match end {
                    Bound::Unbounded => vec.len(),
                    Bound::Included(end) => vec.partition_point(|(range, _v)| range.end <= end),
                    Bound::Excluded(end) => vec.partition_point(|(range, _v)| range.end < end),
                };
                assert_eq!(vec[start.min(end)..end], entries);
            }
            Action::Iter(from) => {
                let (iter, index) = if let Some(from) = from {
                    (
                        tree.iter_from(from),
                        match from {
                            Bound::Unbounded => 0,
                            Bound::Included(from) => vec.partition_point(|(r, _v)| r.end < from),
                            Bound::Excluded(from) => vec.partition_point(|(r, _v)| r.end <= from),
                        },
                    )
                } else {
                    (tree.iter(), 0)
                };

                let entries: Vec<_> = iter.map(|(k, &v)| (k, v)).collect();
                assert_eq!(entries, vec[index..]);
            }
            Action::Gaps => {
                let gaps = tree.gaps();
                let gaps: Vec<_> = gaps.collect();

                let mut expected_gaps: Vec<_> = vec
                    .iter()
                    .scan(Some(Bound::Unbounded), |prev_end, (range, _v)| {
                        let gap = ((*prev_end)?, Bound::Excluded(range.start));

                        *prev_end = range.end.checked_increment().map(Bound::Included);

                        Some(gap)
                    })
                    .collect();

                // add the final gap at the end. either between the last range and MAX or
                // if no ranges exists the gap between ZERO and MAX
                if let Some((last_range, _)) = vec.last() {
                    if let Some(end) = last_range.end.checked_increment() {
                        expected_gaps.push((Bound::Included(end), Bound::Unbounded));
                    }
                } else {
                    expected_gaps.push((Bound::Unbounded, Bound::Unbounded))
                }

                // filter out all empty ranges
                expected_gaps.retain(|gap| !gap.is_empty());

                assert_eq!(gaps, expected_gaps);
            }
            Action::Cursor(at, actions) => {
                let (mut cursor, mut index) = if let Some(at) = at {
                    (
                        tree.cursor_at(at),
                        match at {
                            Bound::Unbounded => vec.len(),
                            Bound::Included(at) => vec.partition_point(|(r, _v)| r.end < at),
                            Bound::Excluded(at) => vec.partition_point(|(r, _v)| r.end <= at),
                        },
                    )
                } else {
                    (tree.cursor(), 0)
                };

                let entries: Vec<_> = cursor.iter().map(|(k, &v)| (k, v)).collect();
                assert_eq!(entries, vec[index..]);

                for action in actions {
                    match action {
                        CursorAction::Next => {
                            if index != vec.len() {
                                cursor.next();
                                index += 1;
                            }
                        }
                        CursorAction::Prev => {
                            let ok = cursor.prev();
                            assert_eq!(ok, index != 0);
                            if ok {
                                index -= 1;
                            }
                        }
                    }

                    assert_eq!(cursor.is_end(), index == vec.len());
                    let entries: Vec<_> = cursor.iter().map(|(k, &v)| (k, v)).collect();
                    assert_eq!(entries, vec[index..]);
                }
            }
            Action::CursorMut(at, actions) => {
                let (mut cursor, mut index) = if let Some(at) = at {
                    (
                        tree.cursor_mut_at(at),
                        match at {
                            Bound::Unbounded => vec.len(),
                            Bound::Included(at) => vec.partition_point(|(r, _v)| r.end < at),
                            Bound::Excluded(at) => vec.partition_point(|(r, _v)| r.end <= at),
                        },
                    )
                } else {
                    (tree.cursor_mut(), 0)
                };

                let entries: Vec<_> = cursor.iter().map(|(k, &v)| (k, v)).collect();
                assert_eq!(entries, vec[index..]);

                for action in actions {
                    match action {
                        CursorMutAction::Next => {
                            if index != vec.len() {
                                cursor.next();
                                index += 1;
                            }
                        }
                        CursorMutAction::Prev => {
                            let ok = cursor.prev();
                            assert_eq!(ok, index != 0);
                            if ok {
                                index -= 1;
                            }
                        }
                        CursorMutAction::Insert { start, end, value } => {
                            let range = if vec.is_empty() {
                                RangeInclusive { start, end }
                            } else if index == vec.len() {
                                vec[index - 1].0.clone()
                            } else {
                                vec[index].0.clone()
                            };
                            cursor.insert(range.clone(), value);
                            vec.insert(index, (range, value));
                        }
                        CursorMutAction::Replace(value) => {
                            if index != vec.len() {
                                let entry = cursor.replace(vec[index].0.clone(), value);
                                let vec_entry = vec[index].clone();
                                assert_eq!(entry, vec_entry);
                                vec[index].1 = value;
                            }
                        }
                        CursorMutAction::Remove => {
                            if index != vec.len() {
                                let entry = cursor.remove();
                                let vec_entry = vec.remove(index);
                                assert_eq!(entry, vec_entry);
                            }
                        }
                    }

                    let entries: Vec<_> = cursor.iter().map(|(k, &v)| (k, v)).collect();
                    assert_eq!(entries, vec[index..]);
                    assert_eq!(
                        cursor.entry().map(|(k, &v)| (k, v)),
                        vec.get(index).cloned()
                    );
                    assert_eq!(cursor.is_end(), index == vec.len());
                }
            }
        }

        assert_eq!(vec.is_empty(), tree.is_empty());
        let btree_entries: Vec<_> = tree.iter().map(|(range, &v)| (range, v)).collect();
        assert_eq!(vec, btree_entries);
    }
}

fn dispatch_by_key<'a, Value: Eq + Arbitrary<'a> + Debug + Copy>(actions: IndexType<Value>) {
    match actions {
        IndexType::U8(actions) => run(actions),
        IndexType::U16(actions) => run(actions),
        IndexType::U32(actions) => run(actions),
        IndexType::U64(actions) => run(actions),
        IndexType::U128(actions) => run(actions),
    }
}

fuzz_target!(|actions: ValueType| {
    match actions {
        ValueType::Empty(actions) => dispatch_by_key(actions),
        ValueType::U8(actions) => dispatch_by_key(actions),
        ValueType::U16(actions) => dispatch_by_key(actions),
        ValueType::U32(actions) => dispatch_by_key(actions),
        ValueType::U64(actions) => dispatch_by_key(actions),
        ValueType::U128(actions) => dispatch_by_key(actions),
    }
});
