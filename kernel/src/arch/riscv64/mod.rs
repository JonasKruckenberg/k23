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
use vmm::{
    AddressRangeExt, BitMapAllocator, BumpAllocator, EntryFlags, Flush, FrameAllocator, FrameUsage,
    Mapper, PhysicalAddress, VirtualAddress,
};

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
pub extern "C" fn kstart(kargs: *const KernelArgs) -> ! {
    let kargs = unsafe { &*(kargs) };
    let boot_info = BootInfo::from_dtb(kargs.fdt.as_raw() as *const u8);

    kernel_mapper::init(&boot_info.memories, kargs.alloc_offset);

    let serial_base = with_kernel_mapper(|mapper, flush| {
        let serial_phys = boot_info.serial.regs.clone().align(kconfig::PAGE_SIZE);
        let serial_virt = {
            let base = kargs.stack_start;

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

    logger::init(serial_base, boot_info.serial.clock_frequency);
    log::debug!("hello world!");

    todo!()
}
