#![no_std]
#![no_main]
#![feature(
    naked_functions,
    asm_const,
    allocator_api,
    thread_local,
    error_in_core,
    hint_assert_unchecked,
    used_with_arg
)]

extern crate alloc;

mod allocator;
mod arch;
mod frame_alloc;
mod logger;
mod runtime;
mod tests;

pub mod kconfig {
    #[allow(non_camel_case_types)]
    #[cfg(target_arch = "riscv64")]
    pub type MEMORY_MODE = vmm::Riscv64Sv39;
    #[allow(non_camel_case_types)]
    #[cfg(not(target_arch = "riscv64"))]
    pub type MEMORY_MODE = vmm::EmulateMode;

    pub const PAGE_SIZE: usize = <MEMORY_MODE as ::vmm::Mode>::PAGE_SIZE;
    pub const HEAP_SIZE_PAGES: usize = 8192; // 32 MiB
    pub const TRAP_STACK_SIZE_PAGES: usize = 16; // Size of the per-hart trap stack in pages
}

fn main(_hartid: usize) -> ! {
    // Eventually this will all be hidden behind other abstractions (the scheduler, etc.) and this
    // function will just jump into the scheduling loop

    todo!()
}

#[no_mangle]
pub static mut __stack_chk_guard: u64 = 0xe57fad0f5f757433;

/// # Safety
///
/// Extern
#[no_mangle]
pub unsafe extern "C" fn __stack_chk_fail() {
    panic!("Kernel stack is corrupted")
}

// #[derive(Debug)]
// pub struct StackUsage {
//     pub used: usize,
//     pub total: usize,
//     pub high_watermark: usize,
// }
//
// impl StackUsage {
//     pub const FILL_PATTERN: u64 = 0xACE0BACE;
//
//     pub fn measure() -> Self {
//         let sp: usize;
//         unsafe {
//             asm!("mv {}, sp", out(reg) sp);
//         }
//         let sp = unsafe { VirtualAddress::new(sp) };
//
//         STACK.with(|stack| {
//             let high_watermark = Self::stack_high_watermark(stack.clone());
//
//             if sp < stack.start {
//                 panic!("stack overflow");
//             }
//
//             StackUsage {
//                 used: stack.end.sub_addr(sp),
//                 total: kconfig::STACK_SIZE_PAGES * kconfig::PAGE_SIZE,
//                 high_watermark: stack.end.sub_addr(high_watermark),
//             }
//         })
//     }
//
//     fn stack_high_watermark(stack_region: Range<VirtualAddress>) -> VirtualAddress {
//         unsafe {
//             let mut ptr = stack_region.start.as_raw() as *const u64;
//             let stack_top = stack_region.end.as_raw() as *const u64;
//
//             while ptr < stack_top && *ptr == Self::FILL_PATTERN {
//                 ptr = ptr.offset(1);
//             }
//
//             VirtualAddress::new(ptr as usize)
//         }
//     }
// }
