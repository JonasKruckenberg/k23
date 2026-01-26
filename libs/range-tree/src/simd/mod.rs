//! SIMD-optimized implementations of binary search in a fixed-length slice.

use core::cmp::Ordering;
use core::fmt::Debug;
use core::hint;

use cfg_if::cfg_if;

use crate::int::PIVOTS_BYTES;

cfg_if! {
    if #[cfg(all(
        any(target_arch = "x86", target_arch = "x86_64"),
        target_feature = "avx512bw",
        target_feature = "popcnt",
    ))] {
        // x86-64-v4: AVX512F + AVX512BW
        mod avx512;
    } else if #[cfg(all(
        any(target_arch = "x86", target_arch = "x86_64"),
        target_feature = "avx2",
        target_feature = "popcnt",
    ))] {
        // x86-64-v3: AVX2 + POPCNT
        mod avx2;
    } else if #[cfg(all(
        any(target_arch = "x86", target_arch = "x86_64"),
        target_feature = "sse2",
        target_feature = "popcnt",
    ))] {
        // x86-64-v2: SSE2 + POPCNT
        mod sse2_popcnt;
    } else if #[cfg(all(
        any(target_arch = "x86", target_arch = "x86_64"),
        target_feature = "sse2",
    ))] {
        // x86-64-v1: SSE2
        mod sse2;
    } else if #[cfg(all(
        target_arch = "aarch64",
        target_feature = "sve",
    ))] {
        // AArch64 SVE
        mod sve;
    } else if #[cfg(all(
        target_arch = "aarch64",
        target_feature = "neon",
    ))] {
        // AArch64 NEON
        mod neon;
    } else if #[cfg(all(
        any(target_arch = "riscv32", target_arch = "riscv64"),
        target_feature = "v",
    ))] {
        // RISC-V RVV
        mod rvv;
    } else {
        // Default fallback implementation using unrolled binary search
        impl SimdSearch for u8 {}
        impl SimdSearch for u16 {}
        impl SimdSearch for u32 {}
        impl SimdSearch for u64 {}
        impl SimdSearch for u128 {}
        impl SimdSearch for i8 {}
        impl SimdSearch for i16 {}
        impl SimdSearch for i32 {}
        impl SimdSearch for i64 {}
        impl SimdSearch for i128 {}
    }
}

/// Helper trait for integers.
pub(crate) trait Int: Ord + Copy + Debug {
    const ZERO: Self;
    fn wrapping_add(self, other: Self) -> Self;
}
macro_rules! impl_zero {
    ($($int:ident,)*) => {
        $(
            impl Int for $int {
                const ZERO: Self = 0;
                #[inline]
                fn wrapping_add(self, other: Self) -> Self {
                    self.wrapping_add(other)
                }
            }
        )*
    };
}
impl_zero! {
    u8,
    u16,
    u32,
    u64,
    u128,
    i8,
    i16,
    i32,
    i64,
    i128,
}

/// SIMD search on an array of sorted integers.
pub(crate) trait SimdSearch: Int {
    /// Number of elements that the SIMD search will process.
    ///
    /// This must be a power of 2.
    const SIMD_WIDTH: usize = 1;

    /// Some architectures (*cough* x86) only support signed SIMD comparisons
    /// so we need to convert unsigned numbers to signed when storing them in
    /// nodes. To keep the ordering correct, we need to apply a bias by adding
    /// `0x8000...` to the integer before writing it to the node and then
    /// subtracting that when reading it.
    const BIAS: Self = Self::ZERO;

    // Bias-aware comparison of 2 integers.
    #[inline]
    fn bias_cmp(a: Self, b: Self) -> Ordering {
        Ord::cmp(&a.wrapping_add(Self::BIAS), &b.wrapping_add(Self::BIAS))
    }

    /// Performs a binary search on sorted elements in `pivots`, returning the
    /// index of the first element greater than or equal to `search`.
    ///
    /// The last element of `pivots` is assumed to have the maximum integer
    /// value and as such the returned index will always be less than
    /// `Self::SIMD_WIDTH`.
    ///
    /// # Safety
    ///
    /// `pivots` must have `Self::SIMD_WIDTH` elements and be aligned to
    /// `pivotS_BYTES` bytes.
    #[inline]
    unsafe fn search(pivots: &[Self], search: Self) -> usize {
        debug_assert!(pivots.len() >= 2);
        debug_assert!(pivots.len() >= Self::SIMD_WIDTH);
        debug_assert!(pivots.len().is_power_of_two());
        debug_assert_eq!(pivots.as_ptr().addr() % PIVOTS_BYTES, 0);

        // If the pivots are larger than the SIMD search size, use binary search
        // to shrink it. If no SIMD implementation is available then this
        // shrinks down to a single element.
        //
        // Since the length is fixed, the binary search is fully unrolled by the
        // compiler and only uses ~3 instructions per iteration.
        let mut len = pivots.len();
        let mut base = 0;

        // This code is based on the binary search implementation in the
        // standard library.
        while len > Self::SIMD_WIDTH {
            let mid = base + len / 2;

            // This is slightly different from a normal binary search:
            // `simd_seach` requires that the last pivot be less than or equal to
            // `search`, so we check the last pivot of the first half. This works
            // because `len` is guaranteed to be a power of 2 and the last pivot
            // is guaranteed to be the maximum integer value.
            //
            // Since elements in a node have a 2/3 chance of being in
            // the first half of the node, this means we have a 2/3 chance of not
            // needing to load the second half of the pivots into cache.
            let pivot = unsafe { *pivots.get_unchecked(mid - 1) };
            base = hint::select_unpredictable(Self::bias_cmp(search, pivot).is_gt(), mid, base);

            len /= 2;
        }

        debug_assert_eq!(len, Self::SIMD_WIDTH);
        debug_assert_eq!(base % Self::SIMD_WIDTH, 0);
        base + unsafe { Self::simd_search(pivots.as_ptr().add(base), search) }
    }

    /// Performs a SIMD search on sorted elements in `pivots`, returning the
    /// index of the first element greater than or equal to `search`.
    ///
    /// The last element of `pivots` is assumed to always be less than or equal to
    /// `search`. This is ensured by the outer binary search and the node
    /// invariant. As such the returned index will always be less than
    /// `Self::WIDTH`.
    ///
    /// # Safety
    ///
    /// `pivots` must have `Self::WIDTH` elements and be aligned to
    /// `size_of::<T>() * Self::WIDTH` bytes.
    #[inline]
    unsafe fn simd_search(pivots: *const Self, search: Self) -> usize {
        // The default implementation relies entirely on the binary search.
        assert_eq!(Self::SIMD_WIDTH, 1);
        debug_assert!(Self::bias_cmp(search, unsafe { pivots.read() }).is_le());
        0
    }
}

/// Helper function used by some implementations which generate bit masks with
/// duplicate bits.
///
/// # Safety
///
/// `b != 0 && a % b == 0`
#[inline]
#[allow(dead_code)]
unsafe fn exact_div_unchecked(a: usize, b: usize) -> usize {
    unsafe {
        // This hint allows LLVM to remove unnecessary bit shifts.
        hint::assert_unchecked(a.is_multiple_of(b));
        a / core::num::NonZero::new_unchecked(b)
    }
}

#[cfg(test)]
mod tests {
    use super::SimdSearch;
    use crate::int::{CacheAligned, PIVOTS_BYTES};

    fn generic_search<T: SimdSearch>(pivots: &[T], search: T) -> usize {
        pivots[..pivots.len() - 1].partition_point(|&pivot| T::bias_cmp(pivot, search).is_lt())
    }

    fn test_search<T: SimdSearch>(encode: impl Fn(usize) -> T, max: T) {
        let len = PIVOTS_BYTES / size_of::<T>();
        let mut pivots: CacheAligned<[T; PIVOTS_BYTES]> = unsafe { std::mem::zeroed() };
        for i in 0..len {
            pivots.0[i] = encode(i & !1);
        }
        pivots.0[len - 1] = max;
        for i in 0..len {
            assert_eq!(generic_search(&pivots.0[..len], encode(i)), unsafe {
                T::search(&pivots.0[..len], encode(i))
            });
        }
    }

    #[test]
    fn test_search_u8() {
        test_search(
            |i| (i as u8).wrapping_add(SimdSearch::BIAS),
            u8::MAX.wrapping_add(SimdSearch::BIAS),
        );
    }
    #[test]
    fn test_search_u16() {
        test_search(
            |i| (i as u16).wrapping_add(SimdSearch::BIAS),
            u16::MAX.wrapping_add(SimdSearch::BIAS),
        );
    }
    #[test]
    fn test_search_u32() {
        test_search(
            |i| (i as u32).wrapping_add(SimdSearch::BIAS),
            u32::MAX.wrapping_add(SimdSearch::BIAS),
        );
    }
    #[test]
    fn test_search_u64() {
        test_search(
            |i| (i as u64).wrapping_add(SimdSearch::BIAS),
            u64::MAX.wrapping_add(SimdSearch::BIAS),
        );
    }
    #[test]
    fn test_search_u128() {
        test_search(
            |i| (i as u128).wrapping_add(SimdSearch::BIAS),
            u128::MAX.wrapping_add(SimdSearch::BIAS),
        );
    }
    #[test]
    fn test_search_i8() {
        test_search(
            |i| (i as i8).wrapping_add(SimdSearch::BIAS),
            i8::MAX.wrapping_add(SimdSearch::BIAS),
        );
    }
    #[test]
    fn test_search_i16() {
        test_search(
            |i| (i as i16).wrapping_add(SimdSearch::BIAS),
            i16::MAX.wrapping_add(SimdSearch::BIAS),
        );
    }
    #[test]
    fn test_search_i32() {
        test_search(
            |i| (i as i32).wrapping_add(SimdSearch::BIAS),
            i32::MAX.wrapping_add(SimdSearch::BIAS),
        );
    }
    #[test]
    fn test_search_i64() {
        test_search(
            |i| (i as i64).wrapping_add(SimdSearch::BIAS),
            i64::MAX.wrapping_add(SimdSearch::BIAS),
        );
    }
    #[test]
    fn test_search_i128() {
        test_search(
            |i| (i as i128).wrapping_add(SimdSearch::BIAS),
            i128::MAX.wrapping_add(SimdSearch::BIAS),
        );
    }
}

#[cfg(feature = "internal_benches")]
mod bench {
    use super::SimdSearch;
    use crate::int::{CacheAligned, PIVOTS_BYTES};

    #[divan::bench(types = [
        u8,
        u16,
        u32,
        u64,
        u128,
        i8,
        i16,
        i32,
        i64,
        i128,
    ])]
    fn search<T: SimdSearch>(bencher: divan::Bencher) {
        // The values don't matter because we use branchless searches.
        let pivots: CacheAligned<[T; PIVOTS_BYTES]> = unsafe { std::mem::zeroed() };
        bencher.bench_local(|| {
            let zero: T = unsafe { std::mem::zeroed() };
            let len = PIVOTS_BYTES / std::mem::size_of::<T>();
            unsafe { T::search(&pivots.0[..len], divan::black_box(zero)) }
        });
    }

    #[divan::bench(types = [
        u8,
        u16,
        u32,
        u64,
        u128,
        i8,
        i16,
        i32,
        i64,
        i128,
    ])]
    fn generic_search<T: SimdSearch>(bencher: divan::Bencher) {
        fn generic_search<T: SimdSearch>(pivots: &[T], search: T) -> usize {
            pivots[..pivots.len() - 1].partition_point(|&pivot| T::bias_cmp(pivot, search).is_lt())
        }

        // The values don't matter because we use branchless searches.
        let pivots: CacheAligned<[T; PIVOTS_BYTES]> = unsafe { std::mem::zeroed() };
        bencher.bench_local(|| {
            let zero: T = unsafe { std::mem::zeroed() };
            let len = PIVOTS_BYTES / std::mem::size_of::<T>();
            generic_search(&pivots.0[..len], divan::black_box(zero))
        });
    }
}
