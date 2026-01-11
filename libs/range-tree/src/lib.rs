// #![no_std]
#![feature(allocator_api)]

extern crate alloc;

mod cursor;
mod idx;
mod iter;
mod node;

use alloc::alloc::Global;
use core::alloc::{AllocError, Allocator};

use idx::Idx;
use node::{NodePool, NodeRef};

pub struct RangeTree<I: Idx, V, A: Allocator = Global> {
    internal: NodePool<I, NodeRef>,
    leaf: NodePool<I, V>,
    root: NodeRef,
    allocator: A,
}

impl<I: Idx, V> RangeTree<I, V> {
    fn try_new() -> Result<Self, AllocError> {
        Self::try_new_in(Global)
    }

    pub fn try_with_capacity(capacity: usize) -> Result<Self, AllocError> {
        Self::try_with_capacity_in(capacity, Global)
    }
}

impl<I: Idx, V, A: Allocator> RangeTree<I, V, A> {
    pub fn try_new_in(allocator: A) -> Result<Self, AllocError> {
        todo!()
    }

    pub fn try_with_capacity_in(capacity: usize, allocator: A) -> Result<Self, AllocError> {
        todo!()
    }

    pub fn is_empty(&self) -> bool {
        todo!()
    }

    pub fn clear(&mut self) {
        todo!()
    }

    pub fn insert(&mut self, range: core::ops::Range<I>, value: V) -> Option<V> {
        todo!()
    }

    // pub fn get(&self, range: core::ops::Range<I>) -> Option<&V> {
    //     todo!()
    // }
    //
    // pub fn get_mut(&mut self, range: core::ops::Range<I>) -> Option<&mut V> {
    //     todo!()
    // }

    // pub fn remove(&mut self, range: core::ops::Range<I>) -> Option<V> {
    //     todo!()
    // }
}

#[cfg(test)]
mod tests {
    use core::hint;

    const RANGES_BYTES: usize = 128;
    const B: usize = RANGES_BYTES / size_of::<(u64, u64)>();

    /// Number of elements that the SIMD search will process.
    ///
    /// neon can process 8 u64s per search iteration
    const SIMD_WIDTH: usize = 4;

    #[repr(align(128))]
    struct AlignedRanges([(u64, u64); B]);

    #[test]
    fn calculate_gaps() {
        let ranges: AlignedRanges = AlignedRanges([
            (0, 10),
            (15, 20),
            (40, 42),
            (80, 99),
            (110, 200),
            (350, 356),
            (401, 460),
            (470, 480)
            // (u64::MAX, u64::MAX),
        ]);

        let res = unsafe { search(&ranges.0, 10) };
        println!("{:?}", res);
    }

    /// Performs a binary search on sorted elements in `ranges`, returning the
    /// index of the first element greater than or equal to `search`.
    unsafe fn search(ranges: &[(u64, u64)], search: u64) -> Option<usize> {
        debug_assert!(ranges.len() >= 2);
        debug_assert!(ranges.len() >= SIMD_WIDTH);
        debug_assert!(ranges.len().is_power_of_two());
        debug_assert_eq!(ranges.as_ptr().addr() % RANGES_BYTES, 0);

        // If the keys are larger than the SIMD search size, use binary search
        // to shrink it. If no SIMD implementation is available then this
        // shrinks down to a single element.
        //
        // Since the length is fixed, the binary search is fully unrolled by the
        // compiler and only uses ~3 instructions per iteration.
        let mut len = ranges.len();
        let mut base = 0;

        // This code is based on the binary search implementation in the
        // standard library.
        while len > SIMD_WIDTH {
            let mid = base + len / 2;

            // This is slightly different from a normal binary search:
            // `simd_seach` requires that the last key be less than or equal to
            // `search`, so we check the last key of the first half. This works
            // because `len` is guaranteed to be a power of 2 and the last key
            // is guaranteed to be the maximum integer value.
            //
            // Since elements in a node have a 2/3 chance of being in
            // the first half of the node, this means we have a 2/3 chance of not
            // needing to load the second half of the keys into cache.
            let (start, end) = unsafe { *ranges.get_unchecked(mid - 1) };
            base = hint::select_unpredictable(Ord::cmp(&search, &start).is_gt(), mid, base);

            len /= 2;
        }

        debug_assert_eq!(len, SIMD_WIDTH);
        debug_assert_eq!(base % SIMD_WIDTH, 0);
        let off = unsafe { simd_search(ranges.as_ptr().add(base).cast::<u64>(), search) };

        println!("off {off}");

        if off % 2 == 0 {
            // even means in-gap
            None
        } else {
            // odd index means in-range
            Some(base + (off / 2))
        }
    }

    unsafe fn simd_search(keys: *const u64, search: u64) -> usize {
        use core::arch::aarch64::*;

        unsafe {
            let search = vdupq_n_u64(search);
            let uint64x2x4_t(a, b, c, d) = vld1q_u64_x4(keys);
            let a = vcgeq_u64(a, search);
            let b = vcgeq_u64(b, search);
            let c = vcgeq_u64(c, search);
            let d = vcgeq_u64(d, search);
            let ab = vpmaxq_u32(vreinterpretq_u32_u64(a), vreinterpretq_u32_u64(b));
            let cd = vpmaxq_u32(vreinterpretq_u32_u64(c), vreinterpretq_u32_u64(d));
            let abcd = vpmaxq_u16(vreinterpretq_u16_u32(ab), vreinterpretq_u16_u32(cd));
            let abcd_low = vmovn_u16(abcd);
            let low = vget_lane_u64(vreinterpret_u64_u8(abcd_low), 0);
            exact_div_unchecked(low.trailing_zeros() as usize, 8)
        }
    }

    #[inline]
    unsafe fn exact_div_unchecked(a: usize, b: usize) -> usize {
        unsafe {
            // This hint allows LLVM to remove unnecessary bit shifts.
            hint::assert_unchecked(a.is_multiple_of(b));
            a / core::num::NonZero::new_unchecked(b)
        }
    }
}
