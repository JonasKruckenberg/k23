mod register;
// mod trap;

use crate::boot_info::BootInfo;
use crate::kernel_mapper::with_kernel_mapper;
use crate::{kconfig, kernel_mapper, logger};
use core::arch::asm;
use core::iter::Map;
use core::marker::PhantomPinned;
use core::mem::MaybeUninit;
use core::ops::Range;
use core::ptr::addr_of;
use spin::{Mutex, Once};
use uart_16550::SerialPort;
use vmm::{AddressRangeExt, BitMapAllocator, BumpAllocator, EntryFlags, Flush, FrameAllocator, FrameUsage, INIT, Mapper, PhysicalAddress, VirtualAddress};

pub fn halt() -> ! {
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}

#[repr(C, align(16))]
#[derive(Debug)]
pub struct KernelArgs {
    boot_hart: usize,
    fdt_virt: VirtualAddress,
    kernel_start: VirtualAddress,
    kernel_end: VirtualAddress,
    stacks_start: VirtualAddress,
    stacks_end: VirtualAddress,
    frame_alloc_offset: usize,
}

#[no_mangle]
pub extern "C" fn kstart(hartid: usize, kargs: *const KernelArgs) -> ! {
    let kargs = unsafe { &*(kargs) };

    static INIT: Once = Once::new();
    
    INIT.call_once(|| {
        logger::init();
        log::debug!("{hartid} {kargs:?}");

        let boot_info = BootInfo::from_dtb(kargs.fdt_virt.as_raw() as *const u8);

        kernel_mapper::init(&boot_info.memories, kargs.frame_alloc_offset);

        let serial_base = with_kernel_mapper(|mapper, flush| {
            let serial_phys = boot_info.serial.regs.clone().align(kconfig::PAGE_SIZE);
            let serial_virt = {
                let base = kargs.stacks_start;

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

    });
    
    // trap::init();

    log::info!("Hello world from hart {hartid}!");

    // log::debug!("{}", unsafe { *(0x10 as *const u8) });

    todo!()
}
