use core::{cmp::Ordering, fmt::Debug, mem, ops::IndexMut};

use crate::{
    RangeTreeIndex,
    node::{NodePos, NodeRef},
    simd::SimdSearch,
    stack::{Height, Stack, max_height},
};

/// Helper function to convert a pivot directly to a raw integer.
#[inline]
pub(crate) fn int_from_pivot<I: RangeTreeIndex>(pivot: I) -> <I::Int as RangeTreeInteger>::Raw {
    pivot.to_int().to_raw()
}

/// Helper function to convert a raw integer to a pivot.
#[inline]
pub(crate) fn pivot_from_int<I: RangeTreeIndex>(int: <I::Int as RangeTreeInteger>::Raw) -> Option<I> {
    I::Int::from_raw(int).map(I::from_int)
}

/// B is selected so that all the pivots fit in 128 bytes (2 cache lines).
pub(crate) const PIVOTS_BYTES: usize = 128;

/// Nodes are aligned to 128 bytes so they fit exactly in cache lines.
#[repr(C, align(128))]
pub(crate) struct CacheAligned<T>(pub(crate) T);

/// This trait covers all operations that are specific to the integer type used
/// as a pivot.
///
/// # Safety
///
/// All items must be implemented as documented.
pub(crate) unsafe trait RangeTreeInteger:
Copy + Debug + Send + Sync + Unpin
{
    /// Number of elements per node, which must be at least 4.
    ///
    /// The number of elements may vary depending on the integer size to fit in
    /// cache lines or to make optimal use of SIMD instructions.
    const B: usize;

    /// Maximum integer value.
    ///
    /// `search` and `cmp` must compare this as larger than any other integer
    /// value.
    const MAX: Self::Raw;

    /// Raw integer type that is actually stored in the tree.
    type Raw: Copy + Eq + Debug + SimdSearch;

    /// Conversion from a `Self` to a raw integer.
    fn to_raw(self) -> Self::Raw;

    /// Conversion from a raw integer to a `Self`.
    fn from_raw(int: Self::Raw) -> Option<Self>;

    /// Compares 2 integers. We don't just use the `Ord` trait here because some
    /// implementations add a bias to the integer values.
    fn cmp(a: Self::Raw, b: Self::Raw) -> Ordering;

    /// Increments a raw integer by 1.
    fn increment(int: Self::Raw) -> Self::Raw;

    /// Array of pivots used for SIMD comparison in `rank`.
    ///
    /// This must have the same layout as `[Self; Self::B]`.
    type pivots;

    /// Returns the index of the first pivot greater than or equal to `search`.
    ///
    /// Because this assumes that pivots are sorted, it can be implemented either
    /// as a binary search or by counting the number of pivots less than `search`.
    ///
    ///  # Safety
    ///
    /// The last pivot must be `Self::MAX`, which guarantees that the returned
    /// position is less than `Self::B`.
    unsafe fn search(pivots: &Self::pivots, search: Self::Raw) -> NodePos<Self>;

    /// Array of `(NodeRef, NodePos)` pairs which can be indexed by a `Height`.
    type Stack: IndexMut<Height<Self>, Output = (NodeRef, NodePos<Self>)> + Default + Clone;
}

macro_rules! impl_int {
    ($($int:ident $nonmax:ident,)*) => {
        $(
            unsafe impl RangeTreeInteger for nonmax::$nonmax {
                const B: usize = PIVOTS_BYTES / mem::size_of::<Self>();

                const MAX: Self::Raw = $int::MAX.wrapping_add(Self::Raw::BIAS);

                type Raw = $int;

                #[inline]
                fn to_raw(self) -> Self::Raw {
                    self.get().wrapping_add(Self::Raw::BIAS)
                }

                #[inline]
                fn from_raw(int: Self::Raw) -> Option<Self> {
                    Self::new(int.wrapping_sub(Self::Raw::BIAS))
                }

                #[inline]
                fn cmp(a: Self::Raw, b: Self::Raw) -> Ordering {
                    Self::Raw::bias_cmp(a, b)
                }

                #[inline]
                fn increment(int: Self::Raw) -> Self::Raw {
                    int.wrapping_add(1)
                }

                type pivots = CacheAligned<[Self::Raw; Self::B]>;

                #[inline]
                unsafe fn search(pivots: &Self::pivots, search: Self::Raw) -> NodePos<Self> {
                    unsafe { NodePos::new_unchecked(Self::Raw::search(&pivots.0, search)) }
                }

                type Stack = Stack<Self, { max_height::<Self>() }>;
            }

            impl RangeTreeIndex for nonmax::$nonmax {
                type Int = Self;
                
                const ZERO: Self = nonmax::$nonmax::ZERO;
                const MAX: Self = nonmax::$nonmax::MAX;

                #[inline]
                fn to_int(self) -> Self::Int {
                    self
                }

                #[inline]
                fn from_int(int: Self::Int) -> Self {
                    int
                }
            }
        )*
    };
}

impl_int! {
    u8 NonMaxU8,
    u16 NonMaxU16,
    u32 NonMaxU32,
    u64 NonMaxU64,
    u128 NonMaxU128,
    i8 NonMaxI8,
    i16 NonMaxI16,
    i32 NonMaxI32,
    i64 NonMaxI64,
    i128 NonMaxI128,
}