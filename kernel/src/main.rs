#![no_std]
#![no_main]

// bring the #[panic_handler] and #[global_allocator] into scope
extern crate kernel as _;

#[no_mangle]
extern "Rust" fn kmain(
    _hartid: usize,
    _boot_info: &'static loader_api::BootInfo
) -> ! {
    log::trace!("kmain");

    kernel::arch::exit(0);
}
