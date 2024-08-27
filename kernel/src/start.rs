use crate::{allocator, arch, frame_alloc, kconfig};
use core::ops::Range;
use kmm::{BitMapAllocator, Flush, Mapper, VirtualAddress};
use loader_api::{BootInfo, LoaderConfig};

const LOADER_CFG: LoaderConfig = {
    let mut cfg = LoaderConfig::new_default();
    cfg.kernel_stack_size_pages = kconfig::STACK_SIZE_PAGES;
    cfg.kernel_heap_size_pages = Some(kconfig::HEAP_SIZE_PAGES);
    cfg
};

#[cfg(not(test))]
#[loader_api::entry(LOADER_CFG)]
fn kstart(hartid: usize, boot_info: &'static BootInfo) -> ! {
    panic::catch_unwind(|| {
        semihosting_logger::hartid::set(hartid);

        arch::trap_handler::init();

        if hartid == boot_info.boot_hart {
            init_global(boot_info);
        }

        arch::finish_processor_init();

        crate::kmain(hartid, boot_info);
    })
    .unwrap_or_else(|_| {
        log::error!("unrecoverable failure");
        arch::abort();
    })
}

#[cfg(test)]
#[ktest::setup_harness(LOADER_CFG)]
fn kstart_test(hartid: usize, info: ktest::SetupInfo) {
    semihosting_logger::hartid::set(hartid);

    arch::trap_handler::init();

    if hartid == info.boot_info.boot_hart {
        init_global(info.boot_info);
    }

    arch::finish_processor_init();
}

fn init_global(boot_info: &'static BootInfo) {
    semihosting_logger::init(kconfig::LOG_LEVEL.to_level_filter());

    log::debug!("initializing frame alloc...");
    frame_alloc::init(boot_info, |alloc| -> Result<(), kmm::Error> {
        log::debug!("Unmapping loader region {:?}...", boot_info.loader_region);
        unmap_loader(alloc, boot_info.loader_region.clone())?;

        log::debug!("Setting up kernel heap...");
        allocator::init(alloc, boot_info)?;

        Ok(())
    })
    .expect("failed to initialize frame allocator");

    log::info!("Welcome to k23 {}", env!("CARGO_PKG_VERSION"));
}

fn unmap_loader(
    alloc: &mut BitMapAllocator<kconfig::MEMORY_MODE>,
    loader_region: Range<VirtualAddress>,
) -> Result<(), kmm::Error> {
    let mut mapper: Mapper<kconfig::MEMORY_MODE> = Mapper::from_active(0, alloc);
    let mut flush = Flush::empty(0);

    // Unmap the loader regions
    mapper.unmap_forget_range(loader_region, &mut flush)?;

    flush.flush()?;

    Ok(())
}
