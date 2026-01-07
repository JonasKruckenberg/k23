use super::{SimdSearch, exact_div_unchecked};
use core::arch::aarch64::*;

impl SimdSearch for u8 {
    const SIMD_WIDTH: usize = 64;
    #[inline]
    #[target_feature(enable = "neon")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_u8(search);
        let uint8x16x4_t(a, b, c, d) = unsafe { vld4q_u8(keys) };
        let a = vcgeq_u8(a, search);
        let b = vcgeq_u8(b, search);
        let c = vcgeq_u8(c, search);
        let d = vcgeq_u8(d, search);
        let ab = vsriq_n_u8(b, a, 1);
        let cd = vsriq_n_u8(d, c, 1);
        let abcd = vsriq_n_u8(cd, ab, 2);
        let abcd = vsriq_n_u8(abcd, abcd, 4);
        let abcd_low = vshrn_n_u16(vreinterpretq_u16_u8(abcd), 4);
        let low = vget_lane_u64(vreinterpret_u64_u8(abcd_low), 0);
        low.trailing_zeros() as usize
    }
}
impl SimdSearch for u16 {
    const SIMD_WIDTH: usize = 32;
    #[inline]
    #[target_feature(enable = "neon")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_u16(search);
        let uint16x8x4_t(a, b, c, d) = unsafe { vld4q_u16(keys) };
        let a = vcgeq_u16(a, search);
        let b = vcgeq_u16(b, search);
        let c = vcgeq_u16(c, search);
        let d = vcgeq_u16(d, search);
        let ab = vsriq_n_u16(b, a, 2);
        let cd = vsriq_n_u16(d, c, 2);
        let abcd = vsriq_n_u16(cd, ab, 4);
        let abcd_low = vshrn_n_u16(abcd, 8);
        let low = vget_lane_u64(vreinterpret_u64_u8(abcd_low), 0);
        unsafe { exact_div_unchecked(low.trailing_zeros() as usize, 2) }
    }
}
impl SimdSearch for u32 {
    const SIMD_WIDTH: usize = 16;
    #[inline]
    #[target_feature(enable = "neon")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_u32(search);
        let uint32x4x4_t(a, b, c, d) = unsafe { vld1q_u32_x4(keys) };
        let a = vcgeq_u32(a, search);
        let b = vcgeq_u32(b, search);
        let c = vcgeq_u32(c, search);
        let d = vcgeq_u32(d, search);
        let ab = vpmaxq_u16(vreinterpretq_u16_u32(a), vreinterpretq_u16_u32(b));
        let cd = vpmaxq_u16(vreinterpretq_u16_u32(c), vreinterpretq_u16_u32(d));
        let abcd = vpmaxq_u8(vreinterpretq_u8_u16(ab), vreinterpretq_u8_u16(cd));
        let abcd_low = vshrn_n_u16(vreinterpretq_u16_u8(abcd), 4);
        let low = vget_lane_u64(vreinterpret_u64_u8(abcd_low), 0);
        unsafe { exact_div_unchecked(low.trailing_zeros() as usize, 4) }
    }
}
impl SimdSearch for u64 {
    const SIMD_WIDTH: usize = 8;
    #[inline]
    #[target_feature(enable = "neon")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_u64(search);
        let uint64x2x4_t(a, b, c, d) = unsafe { vld1q_u64_x4(keys) };
        let a = vcgeq_u64(a, search);
        let b = vcgeq_u64(b, search);
        let c = vcgeq_u64(c, search);
        let d = vcgeq_u64(d, search);
        let ab = vpmaxq_u32(vreinterpretq_u32_u64(a), vreinterpretq_u32_u64(b));
        let cd = vpmaxq_u32(vreinterpretq_u32_u64(c), vreinterpretq_u32_u64(d));
        let abcd = vpmaxq_u16(vreinterpretq_u16_u32(ab), vreinterpretq_u16_u32(cd));
        let abcd_low = vmovn_u16(abcd);
        let low = vget_lane_u64(vreinterpret_u64_u8(abcd_low), 0);
        unsafe { exact_div_unchecked(low.trailing_zeros() as usize, 8) }
    }
}
impl SimdSearch for u128 {}
impl SimdSearch for i8 {
    const SIMD_WIDTH: usize = 64;
    #[inline]
    #[target_feature(enable = "neon")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_s8(search);
        let int8x16x4_t(a, b, c, d) = unsafe { vld4q_s8(keys) };
        let a = vcgeq_s8(a, search);
        let b = vcgeq_s8(b, search);
        let c = vcgeq_s8(c, search);
        let d = vcgeq_s8(d, search);
        let ab = vsriq_n_u8(b, a, 1);
        let cd = vsriq_n_u8(d, c, 1);
        let abcd = vsriq_n_u8(cd, ab, 2);
        let abcd = vsriq_n_u8(abcd, abcd, 4);
        let abcd_low = vshrn_n_u16(vreinterpretq_u16_u8(abcd), 4);
        let low = vget_lane_u64(vreinterpret_u64_u8(abcd_low), 0);
        low.trailing_zeros() as usize
    }
}
impl SimdSearch for i16 {
    const SIMD_WIDTH: usize = 32;
    #[inline]
    #[target_feature(enable = "neon")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_s16(search);
        let int16x8x4_t(a, b, c, d) = unsafe { vld4q_s16(keys) };
        let a = vcgeq_s16(a, search);
        let b = vcgeq_s16(b, search);
        let c = vcgeq_s16(c, search);
        let d = vcgeq_s16(d, search);
        let ab = vsriq_n_u16(b, a, 2);
        let cd = vsriq_n_u16(d, c, 2);
        let abcd = vsriq_n_u16(cd, ab, 4);
        let abcd_low = vshrn_n_u16(abcd, 8);
        let low = vget_lane_u64(vreinterpret_u64_u8(abcd_low), 0);
        unsafe { exact_div_unchecked(low.trailing_zeros() as usize, 2) }
    }
}
impl SimdSearch for i32 {
    const SIMD_WIDTH: usize = 16;
    #[inline]
    #[target_feature(enable = "neon")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_s32(search);
        let int32x4x4_t(a, b, c, d) = unsafe { vld1q_s32_x4(keys) };
        let a = vcgeq_s32(a, search);
        let b = vcgeq_s32(b, search);
        let c = vcgeq_s32(c, search);
        let d = vcgeq_s32(d, search);
        let ab = vpmaxq_u16(vreinterpretq_u16_u32(a), vreinterpretq_u16_u32(b));
        let cd = vpmaxq_u16(vreinterpretq_u16_u32(c), vreinterpretq_u16_u32(d));
        let abcd = vpmaxq_u8(vreinterpretq_u8_u16(ab), vreinterpretq_u8_u16(cd));
        let abcd_low = vshrn_n_u16(vreinterpretq_u16_u8(abcd), 4);
        let low = vget_lane_u64(vreinterpret_u64_u8(abcd_low), 0);
        unsafe { exact_div_unchecked(low.trailing_zeros() as usize, 4) }
    }
}
impl SimdSearch for i64 {
    const SIMD_WIDTH: usize = 8;
    #[inline]
    #[target_feature(enable = "neon")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_s64(search);
        let int64x2x4_t(a, b, c, d) = unsafe { vld1q_s64_x4(keys) };
        let a = vcgeq_s64(a, search);
        let b = vcgeq_s64(b, search);
        let c = vcgeq_s64(c, search);
        let d = vcgeq_s64(d, search);
        let ab = vpmaxq_u32(vreinterpretq_u32_u64(a), vreinterpretq_u32_u64(b));
        let cd = vpmaxq_u32(vreinterpretq_u32_u64(c), vreinterpretq_u32_u64(d));
        let abcd = vpmaxq_u16(vreinterpretq_u16_u32(ab), vreinterpretq_u16_u32(cd));
        let abcd_low = vmovn_u16(abcd);
        let low = vget_lane_u64(vreinterpret_u64_u8(abcd_low), 0);
        unsafe { exact_div_unchecked(low.trailing_zeros() as usize, 8) }
    }
}
impl SimdSearch for i128 {}
