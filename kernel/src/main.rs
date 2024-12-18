#![no_std]
#![no_main]

// bring the #[panic_handler] and #[global_allocator] into scope
extern crate kernel as _;

#[no_mangle]
extern "Rust" fn kmain(_hartid: usize, boot_info: &'static loader_api::BootInfo) -> ! {
    kernel::arch::exit(0);
}
