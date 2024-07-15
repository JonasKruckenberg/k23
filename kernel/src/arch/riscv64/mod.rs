mod trap;

use crate::{allocator, frame_alloc, kconfig, logger};
use arrayvec::ArrayVec;
use kstd::arch::sstatus::FS;
use kstd::arch::{sie, sstatus};
use kstd::declare_thread_local;
use kstd::sync::Once;
use loader_api::MemoryRegionKind;
use vmm::{AddressRangeExt, Flush, Mapper};

pub type EntryFlags = vmm::EntryFlags;

declare_thread_local! {
    pub static HARTID: usize;
}

fn setup(hartid: usize, boot_info: &'static mut loader_api::BootInfo) {
    HARTID.initialize_with(hartid, |_, _| {});

    logger::init();
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

            flush.flush()?;

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

            // Safety: serial_base is derived from BootInfo
            // unsafe { logger::init(serial_virt.start, boot_info.serial.clock_frequency) };
            // mapper.root_table().debug_print_table()?;

            let heap_virt = boot_info
                .free_virt
                .end
                .sub(kconfig::HEAP_SIZE_PAGES * kconfig::PAGE_SIZE)
                ..boot_info.free_virt.end;
            boot_info.free_virt.end = boot_info.free_virt.end.sub(heap_virt.size());

            log::debug!("Setting up kernel heap {heap_virt:?}");
            allocator::init(alloc, heap_virt).unwrap();

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
}

#[cfg(not(test))]
#[loader_api::entry(loader_api::LoaderConfig::new_default())]
fn kstart(hartid: usize, boot_info: &'static mut loader_api::BootInfo) -> ! {
    setup(hartid, boot_info);

    crate::main(hartid)
}

#[cfg(test)]
#[ktest::setup_harness]
fn kstart_test(hartid: usize, info: ktest::SetupInfo) {
    setup(hartid, info.boot_info);
}
