#![no_std]
#![no_main]

// bring the #[panic_handler] and #[global_allocator] into scope
extern crate kernel as _;

#[no_mangle]
extern "Rust" fn kmain(_hartid: usize, _boot_info: &'static loader_api::BootInfo) -> ! {
    // Eventually this will all be hidden behind other abstractions (the scheduler, etc.) and this
    // function will just jump into the scheduling loop

    // use cranelift_codegen::settings::Configurable;
    // use kernel::runtime::{Engine, Linker, Module, Store};

    // let wasm = include_bytes!("../../tests/fib/fib_cpp.wasm");
    //
    // let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
    // let mut b = cranelift_codegen::settings::builder();
    // b.set("opt_level", "speed_and_size").unwrap();
    //
    // let target_isa = isa_builder
    //     .finish(cranelift_codegen::settings::Flags::new(b))
    //     .unwrap();
    //
    // let engine = Engine::new(target_isa);
    //
    // let mut store = Store::new(0, boot_info.physical_memory_offset);
    //
    // let module = Module::from_binary(&engine, &store, wasm);
    // log::info!("{module:#?}");
    //
    // let linker = Linker::new();
    // let instance = linker.instantiate(&mut store, &module);
    // instance.debug_print_vmctx(&store);

    kernel::arch::exit(0);
    // todo!()
}
