use crate::machine_info::{HartLocalMachineInfo, MachineInfo};
use crate::{allocator, arch, logger, vm, HEAP_SIZE_PAGES, LOG_LEVEL, STACK_SIZE_PAGES};
use alloc::boxed::Box;
use alloc::string::String;
use backtrace::{Backtrace, SymbolizeContext};
use core::any::Any;
use core::{mem, slice};
use loader_api::{LoaderConfig, MemoryRegionKind};
use mmu::AddressRangeExt;
use sync::{LazyLock, Mutex, OnceLock};
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
        let fdt = locate_device_tree(boot_info);

        hart_init_early(hartid, fdt);

        sync::Once::default().call_once(|| init(boot_info, fdt));

        hart_init_late();
    })
    .unwrap_or_else(|e| {
        mem::forget(e);
        log::error!("system initialization failed");
        arch::abort();
    });

    extern "Rust" {
        fn kmain(hartid: usize, boot_info: &'static loader_api::BootInfo) -> !;
    }

    panic_unwind::catch_unwind(|| {
        backtrace::__rust_begin_short_backtrace(|| unsafe { kmain(hartid, boot_info) })
    })
    .unwrap_or_else(|e| {
        mem::forget(e);
        log::error!("unrecoverable failure");
        arch::abort();
    })
}

/// Early per-hart initialization.
///
/// This function gets called *before* the rest of the system initialization and can therefore only
/// rely on the environment setup by the `loader`: interrupts are disabled, the initial page table is
/// set up, stacks are initialized and the kernels ELF is fully mapped (including thread-local storage).
///
/// Notably, this function **cannot** use the logger and global allocator yet.
fn hart_init_early(hartid: usize, fdt: *const u8) {
    logger::init_hart(hartid);
    arch::hart_init_early();
    arch::trap_handler::init();

    let minfo = unsafe { HartLocalMachineInfo::from_dtb(hartid, fdt).unwrap() };
    HART_LOCAL_MACHINE_INFO.initialize_with(minfo, |_, _| {});
}

fn init(boot_info: &'static loader_api::BootInfo, fdt: *const u8) {
    logger::init(LOG_LEVEL.to_level_filter());

    BOOT_INFO.get_or_init(|| boot_info);
    log::debug!("\n{boot_info}");

    log::trace!("Setting up kernel heap...");
    allocator::init(boot_info);

    init_panic_hook();

    let minfo = MACHINE_INFO
        .get_or_try_init(|| unsafe { MachineInfo::from_dtb(fdt) })
        .unwrap();
    log::debug!("\n{minfo}");

    log::debug!("Setting up kernel virtual address space...");
    vm::init(boot_info, minfo).unwrap();

    log::info!("Welcome to k23 {}", env!("CARGO_PKG_VERSION"));
}

/// Late per-hart initialization.
///
/// This function gets called *after* the kernel systems are initialized.
fn hart_init_late() {
    arch::hart_init_late();
}

fn locate_device_tree(boot_info: &'static loader_api::BootInfo) -> *const u8 {
    let fdt = boot_info
        .memory_regions()
        .iter()
        .find(|region| region.kind == MemoryRegionKind::FDT)
        .expect("no FDT region");

    boot_info
        .physical_address_offset
        .add(fdt.range.start.as_raw())
        .as_raw() as *const u8
}

static SYMBOLIZE_CONTEXT: LazyLock<Mutex<SymbolizeContext>> = LazyLock::new(|| {
    log::trace!("Setting up symbolize context...");
    let boot_info = BOOT_INFO.get().unwrap();

    let elf = xmas_elf::ElfFile::new(unsafe {
        slice::from_ptr_range(
            boot_info
                .kernel_elf
                .clone()
                .add(boot_info.physical_address_offset.as_raw())
                .as_ptr_range(),
        )
    })
    .unwrap();

    let ctx = SymbolizeContext::new(elf, boot_info.kernel_virt.start.as_raw() as u64).unwrap();

    Mutex::new(ctx)
});

fn init_panic_hook() {
    panic_unwind::set_hook(Box::new(|info| {
        let loc = info.location();
        let msg = payload_as_str(info.payload());

        log::error!("hook hart panicked at {loc}:\n{msg}");

        let ctx = SYMBOLIZE_CONTEXT.lock();
        let backtrace = Backtrace::capture(&ctx);
        log::error!("{backtrace}");
    }));
}

fn payload_as_str(payload: &dyn Any) -> &str {
    if let Some(&s) = payload.downcast_ref::<&'static str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.as_str()
    } else {
        "Box<dyn Any>"
    }
}
