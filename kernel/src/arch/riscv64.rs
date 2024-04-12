use core::arch::asm;
use core::ops::Range;
use vmm::VirtualAddress;

pub fn halt() -> ! {
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}

#[repr(C)]
pub struct KernelArgs {
    boot_hart: usize,
    fdt: VirtualAddress,
    kernel_start: VirtualAddress,
    kernel_end: VirtualAddress,
    stack_start: VirtualAddress,
    stack_end: VirtualAddress,
    alloc_offset: usize,
}

#[no_mangle]
pub extern "C" fn kstart(args: KernelArgs) -> ! {
    todo!()
}
