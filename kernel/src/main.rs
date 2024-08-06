#![no_std]
#![no_main]
#![feature(
    naked_functions,
    asm_const,
    allocator_api,
    thread_local,
    hint_assert_unchecked,
    used_with_arg
)]
#![allow(
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::needless_continue,
    clippy::cast_ptr_alignment, // TODO verify
    clippy::unnecessary_wraps,
    clippy::items_after_statements
)]
#![cfg_attr(test, allow(unused))]

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

#[inline(never)]
fn main(_hartid: usize) -> ! {
    panic!("test panic");

    kstd::arch::abort_internal(1);

    // Eventually this will all be hidden behind other abstractions (the scheduler, etc.) and this
    // function will just jump into the scheduling loop

    // use crate::runtime::{Engine, Linker, Module, Store};
    // use cranelift_codegen::settings::Configurable;

    // let wasm = include_bytes!("../tests/fib-cpp.wasm");

    // let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
    // let mut b = cranelift_codegen::settings::builder();
    // b.set("opt_level", "speed_and_size").unwrap();

    // let target_isa = isa_builder
    //     .finish(cranelift_codegen::settings::Flags::new(b))
    //     .unwrap();

    // let engine = Engine::new(target_isa);

    // let mut store = Store::new(0);

    // let module = Module::from_binary(&engine, &store, wasm);
    // log::debug!("{module:#?}");

    // let linker = Linker::new();
    // let instance = linker.instantiate(&mut store, &module);
    // instance.debug_print_vmctx(&store);

    // todo!()
}

#[no_mangle]
pub static mut __stack_chk_guard: u64 = 0xe57f_ad0f_5f75_7433;

/// # Panics
///
/// This function panics if the stack guard value is not the expected value which means the stack is corrupted.
///
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
