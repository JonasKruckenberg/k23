#![no_std]
#![no_main]

// bring the #[panic_handler] and #[global_allocator] into scope
extern crate kernel as _;

use core::alloc::Layout;
use kernel::machine_info::MachineInfo;
use pmm::VirtualAddress;

#[no_mangle]
extern "Rust" fn kmain(_hartid: usize, _boot_info: &'static loader_api::BootInfo, _minfo: &'static MachineInfo) -> ! {
    log::trace!("kmain");

    let mut kernel_aspace = kernel::vm::KERNEL_ASPACE.get().unwrap().lock();

    // kernel
    kernel_aspace.reserve(
        VirtualAddress::new(0xffffffc0c0000000)..VirtualAddress::new(0xffffffc0c011b5e0),
        pmm::Flags::READ,
    );

    // TLS
    kernel_aspace.reserve(
        VirtualAddress::new(0xffffffc100000000)..VirtualAddress::new(0xffffffc100001000),
        pmm::Flags::READ | pmm::Flags::WRITE,
    );

    // heap
    kernel_aspace.reserve(
        VirtualAddress::new(0xffffffc180000000)..VirtualAddress::new(0xffffffc182000000),
        pmm::Flags::READ | pmm::Flags::WRITE,
    );

    // stacks
    kernel_aspace.reserve(
        VirtualAddress::new(0xffffffc140000000)..VirtualAddress::new(0xffffffc140100000),
        pmm::Flags::READ | pmm::Flags::WRITE,
    );
    
    for _ in 0..50 {
        kernel_aspace.find_spot(Layout::from_size_align(4096, 4096).unwrap(), 27);
    }
    
    kernel::arch::exit(0);
}
