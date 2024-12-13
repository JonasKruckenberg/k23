use crate::{allocator, arch, vm, HEAP_SIZE_PAGES, LOG_LEVEL, STACK_SIZE_PAGES};
use core::{mem, slice};
use loader_api::{LoaderConfig, MemoryRegionKind};
use sync::OnceLock;
use crate::machine_info::MachineInfo;

const LOADER_CFG: LoaderConfig = {
    let mut cfg = LoaderConfig::new_default();
    cfg.kernel_stack_size_pages = STACK_SIZE_PAGES;
    cfg.kernel_heap_size_pages = Some(HEAP_SIZE_PAGES);
    cfg
};

pub static BOOT_INFO: OnceLock<&'static loader_api::BootInfo> = OnceLock::new();
pub static MACHINE_INFO: OnceLock<MachineInfo> = OnceLock::new();

#[loader_api::entry(LOADER_CFG)]
fn start(hartid: usize, boot_info: &'static loader_api::BootInfo) -> ! {
    panic_unwind::catch_unwind(|| {
        pre_init_hart(hartid);
        
        BOOT_INFO.get_or_init(|| boot_info);
        
        MACHINE_INFO.get_or_try_init(|| -> crate::Result<_> {
            let fdt =
                unsafe { slice::from_raw_parts(boot_info.memory_regions, boot_info.memory_regions_len) }
                    .iter()
                    .find(|region| region.kind == MemoryRegionKind::FDT)
                    .expect("no FDT region");
        
            let minfo = unsafe {
                MachineInfo::from_dtb(
                    boot_info
                        .physical_memory_offset
                        .add(fdt.range.start.as_raw())
                        .as_raw() as *const u8,
                )?
            };

            init(boot_info, &minfo);
            
            Ok(minfo)
        }).unwrap();

        post_init_hart();
    })
    .unwrap_or_else(|e| {
        mem::forget(e);
        log::error!("failed to initialize");
        arch::abort();
    });
    
    extern "Rust" {
        fn kmain(hartid: usize, boot_info: &'static loader_api::BootInfo, minfo: &'static MachineInfo) -> !;
    }

    panic_unwind::catch_unwind(|| unsafe { kmain(hartid, boot_info, MACHINE_INFO.get().unwrap()) }).unwrap_or_else(|_| {
        log::error!("unrecoverable failure");
        arch::abort();
    })
}

fn pre_init_hart(hartid: usize) {
    semihosting_logger::hartid::set(hartid);
    // setup trap handler
}

fn init(boot_info: &'static loader_api::BootInfo, minfo: &MachineInfo) {
    semihosting_logger::init(LOG_LEVEL.to_level_filter());
    
    log::trace!("{boot_info:?}");

    log::debug!("Setting up kernel heap...");
    allocator::init(boot_info);

    log::debug!("Setting up kernel virtual address space...");
    vm::init(boot_info, minfo);

    // panic_unwind::set_hook(Box::new(|info| {
    //     let location = info.location();
    //     let msg = payload_as_str(info.payload());
    //
    //     log::error!("hart panicked at {location}:\n{msg}");
    //
    //     // TODO this deadlocks :///
    //     // let elf = unsafe {
    //     //     let start = boot_info
    //     //         .physical_memory_offset
    //     //         .add(boot_info.kernel_elf.start.as_raw())
    //     //         .as_raw() as *const u8;
    //     //     slice::from_raw_parts(start, boot_info.kernel_elf.size())
    //     // };
    //     // let elf = ElfFile64::parse(elf).unwrap();
    //     //
    //     // let ctx =
    //     //     SymbolizeContext::new(elf, boot_info.kernel_image_offset.as_raw() as u64).unwrap();
    //     //
    //     // let backtrace = Backtrace::capture(&ctx);
    //     //
    //     // log::error!("{backtrace}");
    // }));

    log::info!("Welcome to k23 {}", env!("CARGO_PKG_VERSION"));
}

fn post_init_hart() {
    // enable interrupts
}

// fn payload_as_str(payload: &dyn Any) -> &str {
//     if let Some(&s) = payload.downcast_ref::<&'static str>() {
//         s
//     } else if let Some(s) = payload.downcast_ref::<String>() {
//         s.as_str()
//     } else {
//         "Box<dyn Any>"
//     }
// }
