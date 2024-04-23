mod trap;

use crate::boot_info::BootInfo;
use crate::kernel_mapper::with_kernel_mapper;
use crate::thread_local::declare_thread_local;
use crate::{kconfig, kernel_mapper, logger};
use core::arch::asm;
use riscv::register;
use spin::Once;
use vmm::{AddressRangeExt, EntryFlags, VirtualAddress};

pub fn halt() -> ! {
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct KernelArgs {
    boot_hart: u32,
    fdt_virt: VirtualAddress,
    stack_start: VirtualAddress,
    stack_end: VirtualAddress,
    hartmems_virt_start: VirtualAddress,
    frame_alloc_offset: usize,
}

declare_thread_local! {
    pub static HARTID: usize;
}

#[no_mangle]
pub extern "C" fn kstart(hartid: usize, kargs: *const KernelArgs) -> ! {
    let kargs = unsafe { &*(kargs) };

    HARTID.initialize_with(hartid, |_, _| {});

    trap::init();

    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let boot_info = BootInfo::from_dtb(kargs.fdt_virt.as_raw() as *const u8);

        kernel_mapper::init(&boot_info.memories, kargs.frame_alloc_offset);

        let serial_base = with_kernel_mapper(|mapper, flush| {
            let serial_phys = boot_info.serial.regs.clone().align(kconfig::PAGE_SIZE);
            let serial_virt = {
                let base = kargs.hartmems_virt_start;

                base.sub(serial_phys.size())..base
            };
            mapper.map_range_with_flush(
                serial_virt.clone(),
                serial_phys,
                EntryFlags::READ | EntryFlags::WRITE,
                flush,
            )?;

            Ok(serial_virt.start)
        })
        .expect("failed to map serial region");

        // Safety: serial_base is derived from BootInfo
        unsafe { logger::init(serial_base, boot_info.serial.clock_frequency) };
    });

    // Safety: Register access
    unsafe {
        register::sstatus::set_sie();
        register::sie::set_stie();
    }

    todo!()
}

// struct MMIOAlloc {
//     offset: VirtualAddress,
// }
//
// impl MMIOAlloc {
//     pub fn new(offset: VirtualAddress) -> Self {
//         Self { offset }
//     }
//
//     pub fn alloc_pages(&mut self, num_pages: usize) -> VirtualAddress {
//         self.offset = self.offset.sub(num_pages * kconfig::PAGE_SIZE);
//         self.offset
//     }
// }
