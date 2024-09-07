#![no_std]
#![no_main]
#![feature(used_with_arg, naked_functions, thread_local)]
#![feature(allocator_api)]

extern crate alloc;
extern crate panic_unwind;
mod allocator;
mod arch;
mod frame_alloc;
mod kconfig;
mod runtime;
mod start;
mod tests;

use loader_api::BootInfo;

pub fn kmain(_hartid: usize, boot_info: &'static BootInfo) -> ! {
    // Eventually this will all be hidden behind other abstractions (the scheduler, etc.) and this
    // function will just jump into the scheduling loop

    use crate::runtime::{Engine, Linker, Module, Store};
    use cranelift_codegen::settings::Configurable;

    let wasm = include_bytes!("../../tests/fib/fib_cpp.wasm");

    let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
    let mut b = cranelift_codegen::settings::builder();
    b.set("opt_level", "speed_and_size").unwrap();

    let target_isa = isa_builder
        .finish(cranelift_codegen::settings::Flags::new(b))
        .unwrap();

    let engine = Engine::new(target_isa);

    let mut store = Store::new(0, boot_info.physical_memory_offset);

    let module = Module::from_binary(&engine, &store, wasm);
    log::debug!("{module:#?}");

    let linker = Linker::new();
    let instance = linker.instantiate(&mut store, &module);
    instance.debug_print_vmctx(&store);

    todo!()
}
