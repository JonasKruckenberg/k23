use super::SimdSearch;
use core::arch::aarch64::*;
use core::mem;

macro_rules! sve_compare {
    ($search:expr, $data:expr, $w:literal, $cmp:literal) => {
        unsafe {
            let out;
            core::arch::asm!(
                concat!("ptrue p0.", $w, ", vl{vl}"),
                concat!("cmp", $cmp, " p1.", $w, ", p0/z, z1.", $w, ", z0.", $w),
                concat!("cmp", $cmp, " p2.", $w, ", p0/z, z2.", $w, ", z0.", $w),
                concat!("cmp", $cmp, " p3.", $w, ", p0/z, z3.", $w, ", z0.", $w),
                concat!("cmp", $cmp, " p4.", $w, ", p0/z, z4.", $w, ", z0.", $w),
                concat!("cntp {out}, p0, p1.", $w),
                concat!("incp {out}, p2.", $w),
                concat!("incp {out}, p3.", $w),
                concat!("incp {out}, p4.", $w),
                vl = const 16 / mem::size_of::<Self>(),
                in("v0") $search,
                in("v1") $data.0,
                in("v2") $data.1,
                in("v3") $data.2,
                in("v4") $data.3,
                out = out(reg) out,
                out("p0") _,
                out("p1") _,
                out("p2") _,
                out("p3") _,
                out("p4") _,
                options(pure, nomem, nostack)
            );
            out
        }
    }
}

impl SimdSearch for u8 {
    const SIMD_WIDTH: usize = 64;
    #[inline]
    #[target_feature(enable = "sve")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_u8(search);
        let data = unsafe { vld1q_u8_x4(keys) };
        sve_compare!(search, data, "b", "lo")
    }
}
impl SimdSearch for u16 {
    const SIMD_WIDTH: usize = 32;
    #[inline]
    #[target_feature(enable = "sve")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_u16(search);
        let data = unsafe { vld1q_u16_x4(keys) };
        sve_compare!(search, data, "h", "lo")
    }
}
impl SimdSearch for u32 {
    const SIMD_WIDTH: usize = 16;
    #[inline]
    #[target_feature(enable = "sve")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_u32(search);
        let data = unsafe { vld1q_u32_x4(keys) };
        sve_compare!(search, data, "s", "lo")
    }
}
impl SimdSearch for u64 {
    const SIMD_WIDTH: usize = 8;
    #[inline]
    #[target_feature(enable = "sve")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_u64(search);
        let data = unsafe { vld1q_u64_x4(keys) };
        sve_compare!(search, data, "d", "lo")
    }
}
impl SimdSearch for u128 {}
impl SimdSearch for i8 {
    const SIMD_WIDTH: usize = 64;
    #[inline]
    #[target_feature(enable = "sve")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_s8(search);
        let data = unsafe { vld1q_s8_x4(keys) };
        sve_compare!(search, data, "b", "lt")
    }
}
impl SimdSearch for i16 {
    const SIMD_WIDTH: usize = 32;
    #[inline]
    #[target_feature(enable = "sve")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_s16(search);
        let data = unsafe { vld1q_s16_x4(keys) };
        sve_compare!(search, data, "h", "lt")
    }
}
impl SimdSearch for i32 {
    const SIMD_WIDTH: usize = 16;
    #[inline]
    #[target_feature(enable = "sve")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_s32(search);
        let data = unsafe { vld1q_s32_x4(keys) };
        sve_compare!(search, data, "s", "lt")
    }
}
impl SimdSearch for i64 {
    const SIMD_WIDTH: usize = 8;
    #[inline]
    #[target_feature(enable = "sve")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        let search = vdupq_n_s64(search);
        let data = unsafe { vld1q_s64_x4(keys) };
        sve_compare!(search, data, "d", "lt")
    }
}
impl SimdSearch for i128 {}
