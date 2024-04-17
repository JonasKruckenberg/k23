use crate::boot_info::BootInfo;
use crate::{kconfig, logger};
use core::arch::asm;
use vmm::{AddressRangeExt, BitMapAllocator, BumpAllocator, EntryFlags, Mapper, VirtualAddress};

pub fn halt() -> ! {
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}

#[derive(Debug)]
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
pub extern "C" fn kstart(args: *const KernelArgs) -> ! {
    let args = unsafe { &*(args) };

    let boot_info = BootInfo::from_dtb(args.fdt.as_raw() as *const u8);

    let mut alloc: BumpAllocator<kconfig::MEMORY_MODE> =
        unsafe { BumpAllocator::new(&boot_info.memories, args.alloc_offset) };

    let mut mapper = Mapper::from_active(0, &mut alloc);

    let serial_phys = boot_info.serial.regs.clone().align(kconfig::PAGE_SIZE);
    let serial_virt = {
        let base = args.stack_start;

        base.sub(serial_phys.size())..base
    };
    let flush = mapper
        .map_range(
            serial_virt.clone(),
            serial_phys,
            EntryFlags::READ | EntryFlags::WRITE,
        )
        .unwrap();
    flush.flush().unwrap();

    logger::init(serial_virt.start, boot_info.serial.clock_frequency);

    log::debug!("hello world!");

    todo!()
}
