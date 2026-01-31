#[cfg(target_arch = "x86")]
use core::arch::x86::*;
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

use super::SimdSearch;

impl SimdSearch for u8 {
    const SIMD_WIDTH: usize = 128;
    #[inline]
    #[target_feature(enable = "avx512bw")]
    unsafe fn simd_search(pivots: *const Self, search: Self) -> usize {
        let search = _mm512_set1_epi8(search as i8);
        let cmp = |offset: usize| {
            let vec = unsafe { _mm512_load_si512(pivots.cast::<__m512i>().add(offset)) };
            _mm512_cmpgt_epu8_mask(search, vec)
        };
        let a = cmp(0);
        let b = cmp(1);
        a.count_ones() as usize + b.count_ones() as usize
    }
}
impl SimdSearch for u16 {
    const SIMD_WIDTH: usize = 64;
    #[inline]
    #[target_feature(enable = "avx512bw")]
    unsafe fn simd_search(pivots: *const Self, search: Self) -> usize {
        let search = _mm512_set1_epi16(search as i16);
        let cmp = |offset: usize| {
            let vec = unsafe { _mm512_load_si512(pivots.cast::<__m512i>().add(offset)) };
            _mm512_cmpgt_epu16_mask(search, vec)
        };
        let a = cmp(0);
        let b = cmp(1);
        _mm512_kunpackd(a as u64, b as u64).count_ones() as usize
    }
}
impl SimdSearch for u32 {
    const SIMD_WIDTH: usize = 32;
    #[inline]
    #[target_feature(enable = "avx512bw")]
    unsafe fn simd_search(pivots: *const Self, search: Self) -> usize {
        let search = _mm512_set1_epi32(search as i32);
        let cmp = |offset: usize| {
            let vec = unsafe { _mm512_load_si512(pivots.cast::<__m512i>().add(offset)) };
            _mm512_cmpgt_epu32_mask(search, vec)
        };
        let a = cmp(0);
        let b = cmp(1);
        _mm512_kunpackw(a as u32, b as u32).count_ones() as usize
    }
}
impl SimdSearch for u64 {
    const SIMD_WIDTH: usize = 16;
    #[inline]
    #[target_feature(enable = "avx512bw")]
    unsafe fn simd_search(pivots: *const Self, search: Self) -> usize {
        let search = _mm512_set1_epi64(search as i64);
        let cmp = |offset: usize| {
            let vec = unsafe { _mm512_load_si512(pivots.cast::<__m512i>().add(offset)) };
            _mm512_cmpgt_epu64_mask(search, vec)
        };
        let a = cmp(0);
        let b = cmp(1);
        _mm512_kunpackb(a as u16, b as u16).count_ones() as usize
    }
}
impl SimdSearch for u128 {}
