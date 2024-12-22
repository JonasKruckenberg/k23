use crate::machine_info::{HartLocalMachineInfo, MachineInfo};
use crate::{allocator, arch, vm, HEAP_SIZE_PAGES, LOG_LEVEL, STACK_SIZE_PAGES};
use core::{mem, slice};
use loader_api::{LoaderConfig, MemoryRegionKind};
use sync::OnceLock;
use thread_local::declare_thread_local;

const LOADER_CFG: LoaderConfig = {
    let mut cfg = LoaderConfig::new_default();
    cfg.kernel_stack_size_pages = STACK_SIZE_PAGES;
    cfg.kernel_heap_size_pages = Some(HEAP_SIZE_PAGES);
    cfg
};

pub static BOOT_INFO: OnceLock<&'static loader_api::BootInfo> = OnceLock::new();
pub static MACHINE_INFO: OnceLock<MachineInfo> = OnceLock::new();

declare_thread_local!(pub static HART_LOCAL_MACHINE_INFO: HartLocalMachineInfo);

#[loader_api::entry(LOADER_CFG)]
fn start(hartid: usize, boot_info: &'static loader_api::BootInfo) -> ! {
    panic_unwind::catch_unwind(|| {
        let fdt = unsafe {
            let fdt = slice::from_raw_parts(boot_info.memory_regions, boot_info.memory_regions_len)
                .iter()
                .find(|region| region.kind == MemoryRegionKind::FDT)
                .expect("no FDT region");

            boot_info
                .physical_address_offset
                .add(fdt.range.start.as_raw())
                .as_raw() as *const u8
        };

        begin_hart_init(hartid, fdt).unwrap();

        BOOT_INFO.get_or_init(|| {
            // reuse OnceLock for one-time initialization
            init(boot_info, fdt).unwrap();

            boot_info
        });

        finish_hart_init();
    })
    .unwrap_or_else(|e| {
        mem::forget(e);
        log::error!("failed to initialize");
        arch::abort();
    });

    extern "Rust" {
        fn kmain(hartid: usize, boot_info: &'static loader_api::BootInfo) -> !;
    }

    panic_unwind::catch_unwind(|| unsafe { kmain(hartid, boot_info) }).unwrap_or_else(|_| {
        log::error!("unrecoverable failure");
        arch::abort();
    })
}

fn begin_hart_init(hartid: usize, fdt: *const u8) -> crate::Result<()> {
    semihosting_logger::hartid::set(hartid);
    arch::trap_handler::init();

    let minfo = unsafe { HartLocalMachineInfo::from_dtb(hartid, fdt)? };
    HART_LOCAL_MACHINE_INFO.initialize_with(minfo, |_, _| {});

    Ok(())
}

fn init(boot_info: &'static loader_api::BootInfo, fdt: *const u8) -> crate::Result<()> {
    semihosting_logger::init(LOG_LEVEL.to_level_filter());

    log::debug!("\n{boot_info}");

    log::debug!("Setting up kernel heap...");
    allocator::init(boot_info);

    let minfo = MACHINE_INFO.get_or_try_init(|| unsafe { MachineInfo::from_dtb(fdt) })?;
    log::trace!("\n{minfo}");

    log::debug!("Setting up kernel virtual address space...");
    vm::init(boot_info, minfo)?;

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

    Ok(())
}

fn finish_hart_init() {
    arch::finish_hart_init()
}
