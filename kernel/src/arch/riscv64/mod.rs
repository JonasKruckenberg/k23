mod trap;

use crate::boot_info::BootInfo;
use crate::thread_local::declare_thread_local;
use crate::{allocator, frame_alloc, kconfig, logger};
use core::arch::asm;
use core::ops::Range;
use riscv::register;
use riscv::register::sstatus::FS;
use riscv::register::{sie, sstatus};
use sync::Once;
use vmm::{AddressRangeExt, EntryFlags, Flush, Mapper, VirtualAddress};

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
    loader_start: VirtualAddress,
    loader_end: VirtualAddress,
}

declare_thread_local! {
    pub static HARTID: usize;
    pub static STACK: Range<VirtualAddress>;
}

#[no_mangle]
pub extern "C" fn kstart(hartid: usize, kargs: *const KernelArgs) -> ! {
    let kargs = unsafe { &*(kargs) };

    riscv::dbg!(kargs);

    HARTID.initialize_with(hartid, |_, _| {});
    STACK.initialize_with(kargs.stack_start..kargs.stack_end, |_, _| {});

    trap::init();

    static INIT: Once = Once::new();

    INIT.get_or_init(|| {
        let boot_info = BootInfo::from_dtb(kargs.fdt_virt.as_raw() as *const u8);

        frame_alloc::init(
            &boot_info.memories,
            kargs.frame_alloc_offset,
            |alloc| -> Result<(), vmm::Error> {
                let mut mapper: Mapper<kconfig::MEMORY_MODE> = Mapper::from_active(0, alloc);
                let mut flush = Flush::empty(0);
                let mut offset = kargs.page_alloc_offset;

                // Unmap the loader regions
                riscv::hprintln!(
                    "Unmapping loader region {:?}",
                    kargs.loader_start..kargs.loader_end
                );
                mapper.unmap_forget_range(kargs.loader_start..kargs.loader_end, &mut flush)?;

                // Map UART MMIO region
                let serial_phys = boot_info.serial.regs.clone().align(kconfig::PAGE_SIZE);
                let serial_virt = offset.sub(serial_phys.size())..offset;
                offset = offset.sub(serial_virt.size());

                riscv::hprintln!(
                    "Mapping UART mmio region {:?} => {:?}",
                    serial_virt,
                    serial_phys,
                );
                mapper.map_range(
                    serial_virt.clone(),
                    serial_phys,
                    EntryFlags::READ | EntryFlags::WRITE,
                    &mut flush,
                )?;

                // Map the kernel heap
                let heap_phys = {
                    let base = mapper
                        .allocator_mut()
                        .allocate_frames(kconfig::HEAP_SIZE_PAGES)?;
                    base..base.add(kconfig::HEAP_SIZE_PAGES * kconfig::PAGE_SIZE)
                };
                let heap_virt = offset.sub(kconfig::HEAP_SIZE_PAGES * kconfig::PAGE_SIZE)..offset;
                //offset = offset.sub(heap_virt.size());

                riscv::hprintln!(
                    "Mapping kernel heap region {:?} => {:?}",
                    heap_virt,
                    heap_phys,
                );
                mapper.map_range(
                    heap_virt.clone(),
                    heap_phys,
                    EntryFlags::READ | EntryFlags::WRITE,
                    &mut flush,
                )?;

                // 0xffffffd7fde00000..0xffffffd7fdfe8480

                // 0xffffffd7fdf6e000..0xffffffd7fff6e000
                // 0xffffffd7fe36dfc0

                flush.flush()?;

                // Safety: serial_base is derived from BootInfo
                unsafe { logger::init(serial_virt.start, boot_info.serial.clock_frequency) };

                // mapper.root_table().debug_print_table()?;

                allocator::init(heap_virt.start).unwrap();

                Ok(())
            },
        )
        .expect("failed to set up mappings");
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

// fn map_serial_device(
//     serial: &boot_info::Serial,
//     offset: VirtualAddress,
// ) -> Result<VirtualAddress, vmm::Error> {
//     with_mapper(0, |mut mapper, flush| {
//         let serial_phys = serial.regs.clone().align(kconfig::PAGE_SIZE);
//         let serial_virt = offset.sub(serial_phys.size())..offset;
//
//         mapper.map_range_with_flush(
//             serial_virt.clone(),
//             serial_phys,
//             EntryFlags::READ | EntryFlags::WRITE,
//             flush,
//         )?;
//
//         Ok(serial_virt.start)
//     })
// }
