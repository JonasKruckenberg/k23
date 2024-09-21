use crate::{allocator, arch, frame_alloc, kconfig, panic};
use core::mem;
use core::ops::Range;
use kmm::{BitMapAllocator, Flush, Mapper, VirtualAddress};
use loader_api::LoaderConfig;
use sync::OnceLock;

pub static BOOT_INFO: OnceLock<&'static loader_api::BootInfo> = OnceLock::new();

const LOADER_CFG: LoaderConfig = {
    let mut cfg = LoaderConfig::new_default();
    cfg.kernel_stack_size_pages = kconfig::STACK_SIZE_PAGES;
    cfg.kernel_heap_size_pages = Some(kconfig::HEAP_SIZE_PAGES);
    cfg
};

#[loader_api::entry(LOADER_CFG)]
fn start(hartid: usize, boot_info: &'static loader_api::BootInfo) -> ! {
    panic::catch_unwind(|| {
        pre_init_hart(hartid);
        // reuse the OnceLock to also ensure the global initialization is done only once
        BOOT_INFO.get_or_init(|| {
            init(boot_info);
            boot_info
        });
        post_init_hart();
    })
    .unwrap_or_else(|e| {
        mem::forget(e);
        log::error!("failed to initialize");
        arch::abort();
    });

    extern "Rust" {
        fn kmain(hartid: usize, boot_info: &'static loader_api::BootInfo) -> !;
    }

    panic::catch_unwind(|| unsafe { kmain(hartid, boot_info) }).unwrap_or_else(|_| {
        log::error!("unrecoverable failure");
        arch::abort();
    })
}

fn pre_init_hart(hartid: usize) {
    semihosting_logger::hartid::set(hartid);
    // setup trap handler
}

fn init(boot_info: &'static loader_api::BootInfo) {
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

fn post_init_hart() {
    // enable interrupts
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
