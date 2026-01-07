use core::cmp::Ordering;
use core::fmt::Debug;
use core::ops::IndexMut;
use crate::node::{marker, NodePos, NodeRef};
use crate::simd::SimdSearch;
use crate::stack::{Height, Stack, max_height};

pub(crate) const CACHE_LINE: usize = 128;

#[repr(C, align(128))]
pub(crate) struct CacheAligned<T>(pub T);

pub(crate) unsafe trait Idx: Copy + Eq + Debug + Send + Sync + Unpin {
    const B: usize;
    const MAX: Self::Raw;

    type Raw: Copy + Eq + Debug + SimdSearch;

    type Pivots;
    
    fn to_raw(self) -> Self::Raw;
    fn from_raw(int: Self::Raw) -> Option<Self>;
    fn cmp(a: Self::Raw, b: Self::Raw) -> Ordering;

    unsafe fn search(pivots: &Self::Pivots, search: Self::Raw) -> NodePos<Self>;

    /// Array of `(NodeRef, NodePos)` pairs which can be indexed by a `Height`.
    type Stack<V>: IndexMut<Height<Self>, Output = (NodeRef<marker::LeafOrInternal<V>>, NodePos<Self>)> + Default + Clone;
}

macro_rules! impl_int {
    ($($int:ident $nonmax:ident,)*) => {
        $(
            unsafe impl Idx for nonmax::$nonmax {
                const B: usize = CACHE_LINE / size_of::<Self>();

                const MAX: Self::Raw = $int::MAX.wrapping_add(Self::Raw::BIAS);

                type Raw = $int;

                type Pivots = CacheAligned<[Self::Raw; Self::B]>;

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
                
                // // #[inline]
                // // fn increment(int: Self::Raw) -> Self::Raw {
                // //     int.wrapping_add(1)
                // // }

                #[inline]
                unsafe fn search(pivots: &Self::Pivots, search: Self::Raw) -> NodePos<Self> {
                    unsafe { NodePos::new_unchecked(Self::Raw::search(&pivots.0, search)) }
                }
                
                type Stack<V> = Stack<Self, V, { max_height::<Self>() }>;
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
