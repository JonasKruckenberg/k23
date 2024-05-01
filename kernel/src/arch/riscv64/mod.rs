mod trap;

use crate::boot_info::BootInfo;
use crate::kernel_mapper::with_kernel_mapper;
use crate::thread_local::declare_thread_local;
use crate::{allocator, boot_info, kconfig, kernel_mapper, logger};
use core::arch::asm;
use core::ops::Range;
use riscv::register;
use riscv::register::sstatus::FS;
use riscv::register::{sie, sstatus};
use sync::Once;
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
    page_alloc_offset: VirtualAddress,
    frame_alloc_offset: usize,
}

declare_thread_local! {
    pub static HARTID: usize;
    pub static STACK: Range<VirtualAddress>;
}

#[no_mangle]
pub extern "C" fn kstart(hartid: usize, kargs: *const KernelArgs) -> ! {
    let kargs = unsafe { &*(kargs) };

    HARTID.initialize_with(hartid, |_, _| {});
    STACK.initialize_with(kargs.stack_start..kargs.stack_end, |_, _| {});

    trap::init();

    static INIT: Once = Once::new();

    INIT.get_or_init(|| {
        let boot_info = BootInfo::from_dtb(kargs.fdt_virt.as_raw() as *const u8);

        kernel_mapper::init(&boot_info.memories, kargs.frame_alloc_offset);

        let serial_base = map_serial_device(&boot_info.serial, kargs.page_alloc_offset)
            .expect("failed to map serial region");

        // Safety: serial_base is derived from BootInfo
        unsafe { logger::init(serial_base, boot_info.serial.clock_frequency) };

        allocator::init(serial_base).unwrap();
    });

    log::debug!("Hart started");

    // Safety: Register access
    unsafe {
        sstatus::set_sie();
        sstatus::set_fs(FS::Initial);
        sie::set_stie();
    }

    crate::main(hartid)
}

fn map_serial_device(
    serial: &boot_info::Serial,
    offset: VirtualAddress,
) -> Result<VirtualAddress, vmm::Error> {
    with_kernel_mapper(|mut mapper, flush| {
        let serial_phys = serial.regs.clone().align(kconfig::PAGE_SIZE);
        let serial_virt = offset.sub(serial_phys.size())..offset;

        mapper.map_range_with_flush(
            serial_virt.clone(),
            serial_phys,
            EntryFlags::READ | EntryFlags::WRITE,
            flush,
        )?;

        Ok(serial_virt.start)
    })
}
