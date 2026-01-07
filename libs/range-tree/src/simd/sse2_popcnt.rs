use super::SimdSearch;
#[cfg(target_arch = "x86")]
use core::arch::x86::*;
#[cfg(target_arch = "x86_64")]
use core::arch::x86_64::*;

#[inline]
#[target_feature(enable = "sse2")]
unsafe fn search8(keys: *const i8, search: i8) -> usize {
    let search = _mm_set1_epi8(search);
    let cmp = |offset: usize| {
        let vec = unsafe { _mm_load_si128(keys.cast::<__m128i>().add(offset)) };
        _mm_cmpgt_epi8(search, vec)
    };
    let a = cmp(0);
    let b = cmp(1);
    let c = cmp(2);
    let d = cmp(3);
    let mask1 = _mm_movemask_epi8(a) as u16;
    let mask2 = _mm_movemask_epi8(b) as u16;
    let mask3 = _mm_movemask_epi8(c) as u16;
    let mask4 = _mm_movemask_epi8(d) as u16;
    mask1.count_ones() as usize
        + mask2.count_ones() as usize
        + mask3.count_ones() as usize
        + mask4.count_ones() as usize
}

#[inline]
#[target_feature(enable = "sse2")]
unsafe fn search16(keys: *const i16, search: i16) -> usize {
    let search = _mm_set1_epi16(search);
    let cmp = |offset: usize| {
        let vec = unsafe { _mm_load_si128(keys.cast::<__m128i>().add(offset)) };
        _mm_cmpgt_epi16(search, vec)
    };
    let a = cmp(0);
    let b = cmp(1);
    let c = cmp(2);
    let d = cmp(3);
    let ab = _mm_packs_epi16(a, b);
    let cd = _mm_packs_epi16(c, d);
    let mask1 = _mm_movemask_epi8(ab) as u16;
    let mask2 = _mm_movemask_epi8(cd) as u16;
    mask1.count_ones() as usize + mask2.count_ones() as usize
}

#[inline]
#[target_feature(enable = "sse2")]
unsafe fn search32(keys: *const i32, search: i32) -> usize {
    let search = _mm_set1_epi32(search);
    let cmp = |offset: usize| {
        let vec = unsafe { _mm_load_si128(keys.cast::<__m128i>().add(offset)) };
        _mm_cmpgt_epi32(search, vec)
    };
    let a = cmp(0);
    let b = cmp(1);
    let c = cmp(2);
    let d = cmp(3);
    // pblendw is slightly more efficient than packssdw on some CPUs. The order
    // is different but it doesn't matter here.
    let (ab, cd) = if cfg!(target_feature = "sse4.1") {
        unsafe {
            (
                _mm_blend_epi16(a, b, 0b01010101),
                _mm_blend_epi16(c, d, 0b01010101),
            )
        }
    } else {
        (_mm_packs_epi32(a, b), _mm_packs_epi32(c, d))
    };
    let abcd = _mm_packs_epi16(ab, cd);
    let mask1 = _mm_movemask_epi8(abcd) as u16;
    mask1.count_ones() as usize
}

// 64-bit compares require SSE4.2
#[inline]
#[cfg(target_feature = "sse4.2")]
#[target_feature(enable = "sse4.2")]
unsafe fn search64(keys: *const i64, search: i64) -> usize {
    let search = _mm_set1_epi64x(search);
    let cmp = |offset: usize| {
        let vec = unsafe { _mm_load_si128(keys.cast::<__m128i>().add(offset)) };
        _mm_cmpgt_epi64(search, vec)
    };
    let a = cmp(0);
    let b = cmp(1);
    let c = cmp(2);
    let d = cmp(3);
    let ab = _mm_blend_epi16(a, b, 0b00110011);
    let cd = _mm_blend_epi16(c, d, 0b00110011);
    let abcd = _mm_blend_epi16(ab, cd, 0b01010101);
    let mask = _mm_movemask_epi8(abcd) as u16;
    unsafe { super::exact_div_unchecked(mask.count_ones() as usize, 2) }
}

impl SimdSearch for u8 {
    const SIMD_WIDTH: usize = 64;
    const BIAS: Self = i8::MIN as Self;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search8(keys.cast(), search as i8) }
    }
}
impl SimdSearch for u16 {
    const SIMD_WIDTH: usize = 32;
    const BIAS: Self = i16::MIN as Self;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search16(keys.cast(), search as i16) }
    }
}
impl SimdSearch for u32 {
    const SIMD_WIDTH: usize = 16;
    const BIAS: Self = i32::MIN as Self;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search32(keys.cast(), search as i32) }
    }
}
impl SimdSearch for u64 {
    #[cfg(target_feature = "sse4.2")]
    const SIMD_WIDTH: usize = 8;
    #[cfg(target_feature = "sse4.2")]
    const BIAS: Self = i64::MIN as Self;
    #[inline]
    #[cfg(target_feature = "sse4.2")]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search64(keys.cast(), search as i64) }
    }
}
impl SimdSearch for u128 {}
impl SimdSearch for i8 {
    const SIMD_WIDTH: usize = 64;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search8(keys, search) }
    }
}
impl SimdSearch for i16 {
    const SIMD_WIDTH: usize = 32;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search16(keys, search) }
    }
}
impl SimdSearch for i32 {
    const SIMD_WIDTH: usize = 16;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search32(keys, search) }
    }
}
impl SimdSearch for i64 {
    #[cfg(target_feature = "sse4.2")]
    const SIMD_WIDTH: usize = 8;
    #[cfg(target_feature = "sse4.2")]
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { search64(keys, search) }
    }
}
impl SimdSearch for i128 {}
