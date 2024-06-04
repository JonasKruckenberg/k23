mod trap;

use crate::thread_local::declare_thread_local;
use crate::{allocator, frame_alloc, kconfig, logger};
use arrayvec::ArrayVec;
use core::arch::asm;
use loader_api::{LoaderConfig, MemoryRegionKind};
use riscv::register;
use riscv::register::sstatus::FS;
use riscv::register::{sie, sstatus};
use sync::Once;
use vmm::{AddressRangeExt, EntryFlags, Flush, Mapper};

pub fn halt() -> ! {
    unsafe {
        loop {
            asm!("wfi");
        }
    }
}

declare_thread_local! {
    pub static HARTID: usize;
    pub static TTEST: usize = const { 42 };
}

#[loader_api::entry(LoaderConfig::new_default())]
fn kstart(hartid: usize, boot_info: &'static mut loader_api::BootInfo) -> ! {
    HARTID.initialize_with(hartid, |_, _| {});

    logger::init();

    TTEST.with(|g| {
        log::info!("Hello World from kernel {:?} {g}", boot_info);
    });

    trap::init();

    static INIT: Once = Once::new();
    INIT.get_or_init(|| {
        let mut usable = ArrayVec::<_, 16>::new();

        for region in boot_info.memory_regions.iter() {
            if region.kind == MemoryRegionKind::Usable {
                usable.push(region.range.clone());
            }
        }

        log::trace!("initializing frame alloc");
        frame_alloc::init(&usable, |alloc| -> Result<(), vmm::Error> {
            let mut mapper: Mapper<kconfig::MEMORY_MODE> = Mapper::from_active(0, alloc);
            let mut flush = Flush::empty(0);

            // Unmap the loader regions
            log::debug!("Unmapping loader region {:?}", boot_info.loader_virt);
            mapper.unmap_forget_range(
                boot_info
                    .loader_virt
                    .clone()
                    .expect("loader has to be mapped"),
                &mut flush,
            )?;

            // // Map UART MMIO region
            // let serial_phys = boot_info.serial.regs.clone().align(kconfig::PAGE_SIZE);
            // let serial_virt = offset.sub(serial_phys.size())..offset;
            // offset = offset.sub(serial_virt.size());
            // riscv::hprintln!(
            //     "Mapping UART mmio region {:?} => {:?}",
            //     serial_virt,
            //     serial_phys,
            // );
            // mapper.map_range(
            //     serial_virt.clone(),
            //     serial_phys,
            //     EntryFlags::READ | EntryFlags::WRITE,
            //     &mut flush,
            // )?;

            // Map the kernel heap
            let heap_phys = {
                let base = mapper
                    .allocator_mut()
                    .allocate_frames(kconfig::HEAP_SIZE_PAGES)?;
                base..base.add(kconfig::HEAP_SIZE_PAGES * kconfig::PAGE_SIZE)
            };

            let heap_virt = boot_info
                .free_virt
                .end
                .sub(kconfig::HEAP_SIZE_PAGES * kconfig::PAGE_SIZE)
                ..boot_info.free_virt.end;
            boot_info.free_virt.end = boot_info.free_virt.end.sub(heap_virt.size());

            log::debug!(
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

            flush.flush()?;

            // Safety: serial_base is derived from BootInfo
            // unsafe { logger::init(serial_virt.start, boot_info.serial.clock_frequency) };
            // mapper.root_table().debug_print_table()?;

            allocator::init(heap_virt.start).unwrap();

            Ok(())
        })
        .expect("failed to set up mappings");
    });

    log::info!("Hart started");

    // Safety: Register access
    unsafe {
        sstatus::set_sie();
        sstatus::set_fs(FS::Initial);
        sie::set_stie();
    }

    crate::main(hartid)
}
