use super::SimdSearch;
use crate::int::KEYS_BYTES;
use core::mem;

/// Returns the runtime vector length in bytes.
#[inline]
fn vlenb() -> usize {
    unsafe {
        let out;
        core::arch::asm!(
            "csrr {out}, vlenb",
            out = out(reg) out,
            options(nomem, nostack, pure, preserves_flags)
        );
        out
    }
}

macro_rules! rvv_search {
    ($ptr:expr, $search:expr, $elen:literal, $cmp:literal) => {{
        let out: usize;
        // Use an LMUL of 4 if the CPU supports 256-bit vectors. This reduces
        // the amount of work the CPU has to do and the branch will almost
        // always be predicted correctly.
        if vlenb() >= 32 {
            core::arch::asm!(
                concat!("vsetvli zero, {vl}, ", $elen, ", m4, ta, ma"),
                concat!("vl", $elen, ".v v8, ({ptr})"),
                concat!("vms", $cmp, ".vx v8, v8, {search}"),
                "vcpop.m {out}, v8",
                vl = in(reg) KEYS_BYTES / mem::size_of::<Self>(),
                ptr = in(reg) $ptr,
                search = in(reg) $search,
                out = lateout(reg) out,
                out("v8") _,
                out("v9") _,
                out("v10") _,
                out("v11") _,
                options(pure, readonly, nostack)
            );
        } else {
            core::arch::asm!(
                concat!("vsetvli zero, {vl}, ", $elen, ", m8, ta, ma"),
                concat!("vl", $elen, ".v v8, ({ptr})"),
                concat!("vms", $cmp, ".vx v8, v8, {search}"),
                "vcpop.m {out}, v8",
                vl = in(reg) KEYS_BYTES / mem::size_of::<Self>(),
                ptr = in(reg) $ptr,
                search = in(reg) $search,
                out = lateout(reg) out,
                out("v8") _,
                out("v9") _,
                out("v10") _,
                out("v11") _,
                out("v12") _,
                out("v13") _,
                out("v14") _,
                out("v15") _,
                options(pure, readonly, nostack)
            );
        }
        out
    }}
}

impl SimdSearch for u8 {
    const SIMD_WIDTH: usize = 128;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { rvv_search!(keys, search, "e8", "ltu") }
    }
}
impl SimdSearch for u16 {
    const SIMD_WIDTH: usize = 64;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { rvv_search!(keys, search, "e16", "ltu") }
    }
}
impl SimdSearch for u32 {
    const SIMD_WIDTH: usize = 32;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { rvv_search!(keys, search, "e32", "ltu") }
    }
}
impl SimdSearch for u64 {
    const SIMD_WIDTH: usize = 16;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { rvv_search!(keys, search, "e64", "ltu") }
    }
}
impl SimdSearch for u128 {}
impl SimdSearch for i8 {
    const SIMD_WIDTH: usize = 128;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { rvv_search!(keys, search, "e8", "lt") }
    }
}
impl SimdSearch for i16 {
    const SIMD_WIDTH: usize = 64;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { rvv_search!(keys, search, "e16", "lt") }
    }
}
impl SimdSearch for i32 {
    const SIMD_WIDTH: usize = 32;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { rvv_search!(keys, search, "e32", "lt") }
    }
}
impl SimdSearch for i64 {
    const SIMD_WIDTH: usize = 16;
    #[inline]
    unsafe fn simd_search(keys: *const Self, search: Self) -> usize {
        unsafe { rvv_search!(keys, search, "e64", "lt") }
    }
}
impl SimdSearch for i128 {}
