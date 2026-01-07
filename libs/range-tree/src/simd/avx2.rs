use super::{SimdSearch, exact_div_unchecked};
#[cfg(target_arch = "x86")]
use core::arch::x86::*;
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

#[inline]
#[target_feature(enable = "avx2")]
unsafe fn search8(keys: *const i8, search: i8) -> usize {
    let search = _mm256_set1_epi8(search);
    let cmp = |offset: usize| {
        let vec = unsafe { _mm256_load_si256(keys.cast::<__m256i>().add(offset)) };
        _mm256_cmpgt_epi8(search, vec)
    };
    let a = cmp(0);
    let b = cmp(1);
    let c = cmp(2);
    let d = cmp(3);
    let mask1 = _mm256_movemask_epi8(a);
    let mask2 = _mm256_movemask_epi8(b);
    let mask3 = _mm256_movemask_epi8(c);
    let mask4 = _mm256_movemask_epi8(d);
    mask1.count_ones() as usize
        + mask2.count_ones() as usize
        + mask3.count_ones() as usize
        + mask4.count_ones() as usize
}

#[inline]
#[target_feature(enable = "avx2")]
unsafe fn search16(keys: *const i16, search: i16) -> usize {
    let search = _mm256_set1_epi16(search);
    let cmp = |offset: usize| {
        let vec = unsafe { _mm256_load_si256(keys.cast::<__m256i>().add(offset)) };
        _mm256_cmpgt_epi16(search, vec)
    };
    let a = cmp(0);
    let b = cmp(1);
    let c = cmp(2);
    let d = cmp(3);
    let ab = _mm256_packs_epi16(a, b);
    let cd = _mm256_packs_epi16(c, d);
    let mask1 = _mm256_movemask_epi8(ab);
    let mask2 = _mm256_movemask_epi8(cd);
    mask1.count_ones() as usize + mask2.count_ones() as usize
}

#[inline]
#[target_feature(enable = "avx2")]
unsafe fn search32(keys: *const i32, search: i32) -> usize {
    let search = _mm256_set1_epi32(search);
    let cmp = |offset: usize| {
        let vec = unsafe { _mm256_load_si256(keys.cast::<__m256i>().add(offset)) };
        _mm256_cmpgt_epi32(search, vec)
    };
    let a = cmp(0);
    let b = cmp(1);
    let c = cmp(2);
    let d = cmp(3);
    let ab = _mm256_blend_epi16(a, b, 0b01010101);
    let cd = _mm256_blend_epi16(c, d, 0b01010101);
    let abcd = _mm256_packs_epi16(ab, cd);
    let mask = _mm256_movemask_epi8(abcd);
    mask.count_ones() as usize
}

#[inline]
#[target_feature(enable = "avx2")]
unsafe fn search64(keys: *const i64, search: i64) -> usize {
    let search = _mm256_set1_epi64x(search);
    let cmp = |offset: usize| {
        let vec = unsafe { _mm256_load_si256(keys.cast::<__m256i>().add(offset)) };
        _mm256_cmpgt_epi64(search, vec)
    };
    let a = cmp(0);
    let b = cmp(1);
    let c = cmp(2);
    let d = cmp(3);
    let ab = _mm256_blend_epi32(a, b, 0b01010101);
    let cd = _mm256_blend_epi32(c, d, 0b01010101);
    let abcd = _mm256_blend_epi16(ab, cd, 0b01010101);
    let mask = _mm256_movemask_epi8(abcd);
    unsafe { exact_div_unchecked(mask.count_ones() as usize, 2) }
}

impl SimdSearch for u8 {
    const SIMD_WIDTH: usize = 128;
    const BIAS: Self = i8::MIN as Self;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search8(keys.cast(), search as i8) }
    }
}
impl SimdSearch for u16 {
    const SIMD_WIDTH: usize = 64;
    const BIAS: Self = i16::MIN as Self;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search16(keys.cast(), search as i16) }
    }
}
impl SimdSearch for u32 {
    const SIMD_WIDTH: usize = 32;
    const BIAS: Self = i32::MIN as Self;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search32(keys.cast(), search as i32) }
    }
}
impl SimdSearch for u64 {
    const SIMD_WIDTH: usize = 16;
    const BIAS: Self = i64::MIN as Self;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search64(keys.cast(), search as i64) }
    }
}
impl SimdSearch for u128 {}
impl SimdSearch for i8 {
    const SIMD_WIDTH: usize = 128;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search8(keys, search) }
    }
}
impl SimdSearch for i16 {
    const SIMD_WIDTH: usize = 64;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search16(keys, search) }
    }
}
impl SimdSearch for i32 {
    const SIMD_WIDTH: usize = 32;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search32(keys, search) }
    }
}
impl SimdSearch for i64 {
    const SIMD_WIDTH: usize = 16;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search64(keys, search) }
    }
}
impl SimdSearch for i128 {}
