#![no_std]
#![no_main]

// bring the #[panic_handler] and #[global_allocator] into scope
extern crate kernel as _;

#[no_mangle]
extern "Rust" fn kmain(_hartid: usize, _boot_info: &'static loader_api::BootInfo) -> ! {
    // Eventually this will all be hidden behind other abstractions (the scheduler, etc.) and this
    // function will just jump into the scheduling loop

    kernel::arch::exit(0);
}
