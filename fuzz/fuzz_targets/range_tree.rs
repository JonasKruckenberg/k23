#![feature(allocator_api)]
#![no_main]

use std::alloc::Global;
use std::fmt::Debug;
use std::ops::Bound;
use std::{iter, ops};

use libfuzzer_sys::arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use range_tree::InsertError::Overlap;
use range_tree::nonmax::*;
use range_tree::{RangeTree, RangeTreeIndex};

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash)]
struct Index<Int>(Int);

macro_rules! impl_index {
    ($($int:ident $nonmax:ident),*) => {
        $(
            impl Arbitrary<'_> for Index<$nonmax> {
                fn arbitrary(u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<Self> {
                    Ok(Index(
                        $nonmax::new(u.int_in_range(0..=$int::MAX - 1)?).unwrap()
                    ))
                }
            }

            impl RangeTreeIndex for Index<$nonmax> {
                type Int = $nonmax;

                const ZERO: Self = Index(<$nonmax>::ZERO);
                const MAX: Self = Index(<$nonmax>::MAX);

                fn from_int(int: Self::Int) -> Self {
                    Self(int)
                }

                fn to_int(self) -> Self::Int {
                    self.0
                }
            }
        )*
    };
}

impl_index! {
    u8 NonMaxU8,
    u16 NonMaxU16,
    u32 NonMaxU32,
    u64 NonMaxU64,
    u128 NonMaxU128,
    i8 NonMaxI8,
    i16 NonMaxI16,
    i32 NonMaxI32,
    i64 NonMaxI64,
    i128 NonMaxI128
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
    InsertBefore {
        start: Index,
        end: Index,
        value: Value,
    },
    InsertAfter(Value),
    Replace(Value),
    Remove,
}

#[derive(Arbitrary, Debug)]
enum IndexType<Value> {
    U8(Vec<Action<Index<NonMaxU8>, Value>>),
    U16(Vec<Action<Index<NonMaxU16>, Value>>),
    U32(Vec<Action<Index<NonMaxU32>, Value>>),
    U64(Vec<Action<Index<NonMaxU64>, Value>>),
    U128(Vec<Action<Index<NonMaxU128>, Value>>),
    I8(Vec<Action<Index<NonMaxI8>, Value>>),
    I16(Vec<Action<Index<NonMaxI16>, Value>>),
    I32(Vec<Action<Index<NonMaxI32>, Value>>),
    I64(Vec<Action<Index<NonMaxI64>, Value>>),
    I128(Vec<Action<Index<NonMaxI128>, Value>>),
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
    Index: Ord + RangeTreeIndex + Arbitrary<'a> + Debug,
    Value: Eq + Arbitrary<'a> + Debug + Copy,
>(
    actions: Vec<Action<Index, Value>>,
) {
    let mut tree = RangeTree::try_new_in(Global).unwrap();
    let mut vec: Vec<(ops::Range<Index>, Value)> = vec![];

    for action in actions {
        match action {
            Action::Clear => {
                tree.clear();
                vec.clear();
            }
            Action::Insert { start, end, value } => {
                let res = tree.insert(start..end, value);

                let index = vec.partition_point(|(range, _v)| range.end < end);

                if index != vec.len() && vec[index].0.start < end {
                    assert_eq!(res, Err(Overlap));
                } else if index != 0 && vec[index - 1].0.end > start {
                    assert_eq!(res, Err(Overlap));
                } else {
                    vec.insert(index, (start..end, value));
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

                let expected_gaps: Vec<_> = vec
                    .iter()
                    // starting at ZERO, produce all gaps between ranges
                    .scan(Index::ZERO, |prev_end, (range, _v)| {
                        let gap = *prev_end..range.start;
                        *prev_end = range.end;
                        Some(gap)
                    })
                    // add the final gap at the end. either between the last range and MAX or
                    // if no ranges exists the gap between ZERO and MAX
                    .chain(if let Some((last_range, _)) = vec.last() {
                        iter::once(last_range.end..Index::MAX)
                    } else {
                        iter::once(Index::ZERO..Index::MAX)
                    })
                    // filter out all empty ranges
                    .filter(|range| !range.is_empty())
                    .collect();

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
                        CursorMutAction::InsertBefore { start, end, value } => {
                            let range = if vec.is_empty() {
                                start..end
                            } else if index == vec.len() {
                                vec[index - 1].0.clone()
                            } else {
                                vec[index].0.clone()
                            };
                            cursor.insert_before(range.clone(), value).unwrap();
                            vec.insert(index, (range, value));
                        }
                        CursorMutAction::InsertAfter(value) => {
                            if index != vec.len() {
                                let key = vec[index].0.clone();
                                cursor.insert_after(key.clone(), value).unwrap();
                                vec.insert(index + 1, (key, value));
                            }
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
        IndexType::I8(actions) => run(actions),
        IndexType::I16(actions) => run(actions),
        IndexType::I32(actions) => run(actions),
        IndexType::I64(actions) => run(actions),
        IndexType::I128(actions) => run(actions),
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
