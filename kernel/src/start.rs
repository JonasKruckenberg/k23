use crate::backtrace::Backtrace;
use crate::mem::PhysicalAddress;
use crate::mem::bootstrap_alloc::BootstrapAllocator;
use crate::{allocator, arch};
use arrayvec::ArrayVec;
use core::ffi::c_void;
use core::range::Range;
use core::slice;
use loader_api::{BootInfo, LoaderConfig, MemoryRegionKind};
use spin::Once;

#[used(linker)]
#[unsafe(link_section = ".loader_config")]
static LOADER_CONFIG: LoaderConfig = {
    let mut cfg = LoaderConfig::new_default();
    cfg.kernel_stack_size_pages = crate::constants::STACK_SIZE_PAGES;
    cfg
};

#[unsafe(no_mangle)]
fn _start(cpuid: usize, boot_info: &'static BootInfo, boot_ticks: u64) -> ! {
    #[used(linker)]
    #[unsafe(link_section = ".eh_frame")]
    static mut EH_FRAME: [u8; 0] = [];

    unsafe extern "C" {
        fn __register_frame(fde: *const c_void);
        fn __deregister_frame(fde: *const c_void);
    }

    #[allow(static_mut_refs)]
    unsafe {
        __register_frame(EH_FRAME.as_ptr().cast())
    }

    panic_unwind::set_hook(|info| {
        tracing::error!("CPU {info}");

        // FIXME 32 seems adequate for unoptimized builds where the callstack can get quite deep
        //  but (at least at the moment) is absolute overkill for optimized builds. Sadly there
        //  is no good way to do conditional compilation based on the opt-level.
        const MAX_BACKTRACE_FRAMES: usize = 32;

        let backtrace = backtrace::__rust_end_short_backtrace(|| {
            Backtrace::<MAX_BACKTRACE_FRAMES>::capture().unwrap()
        });
        tracing::error!("{}", backtrace);

        if backtrace.frames_omitted {
            tracing::warn!("Stack trace was larger than backtrace buffer, omitted some frames.");
        }
    });

    // Unwinding expects at least one landing pad in the callstack, but capturing all unwinds that
    // bubble up to this point is also a good idea since we can perform some last cleanup and
    // print an error message.
    let res = panic_unwind::catch_unwind(|| {
        backtrace::__rust_begin_short_backtrace(|| {
            static GLOBAL_INIT: Once = Once::new();
            GLOBAL_INIT.call_once(|| global_init(boot_info));

            crate::main(cpuid, boot_info, boot_ticks)
        });
    });

    #[allow(static_mut_refs)]
    unsafe {
        __deregister_frame(EH_FRAME.as_ptr().cast())
    }

    match res {
        Ok(_) => arch::exit(0),
        // If the panic propagates up to this catch here there is nothing we can do, this is a terminal
        // failure.
        Err(_) => {
            tracing::error!("unrecoverable kernel panic");
            arch::abort()
        }
    }
}

fn global_init(boot_info: &'static BootInfo) {
    // set up the basic functionality of the tracing subsystem as early as possible
    // tracing::init_early();

    // initialize a simple bump allocator for allocating memory before our virtual memory subsystem
    // is available
    let allocatable_memories = allocatable_memory_regions(boot_info);
    tracing::info!("allocatable memories: {:?}", allocatable_memories);
    let mut boot_alloc = BootstrapAllocator::new(&allocatable_memories);

    // initializing the global allocator
    allocator::init(&mut boot_alloc, boot_info);
}

/// Builds a list of memory regions from the boot info that are usable for allocation.
///
/// The regions passed by the loader are guaranteed to be non-overlapping, but might not be
/// sorted and might not be optimally "packed". This function will both sort regions and
/// attempt to compact the list by merging adjacent regions.
fn allocatable_memory_regions(boot_info: &BootInfo) -> ArrayVec<Range<PhysicalAddress>, 16> {
    let temp: ArrayVec<Range<PhysicalAddress>, 16> = boot_info
        .memory_regions
        .iter()
        .filter_map(|region| {
            let range = Range::from(
                PhysicalAddress::new(region.range.start)..PhysicalAddress::new(region.range.end),
            );

            region.kind.is_usable().then_some(range)
        })
        .collect();

    // merge adjacent regions
    let mut out: ArrayVec<Range<PhysicalAddress>, 16> = ArrayVec::new();
    'outer: for region in temp {
        for other in &mut out {
            if region.start == other.end {
                other.end = region.end;
                continue 'outer;
            }
            if region.end == other.start {
                other.start = region.start;
                continue 'outer;
            }
        }

        out.push(region);
    }

    out
}

fn locate_device_tree(boot_info: &BootInfo) -> (&'static [u8], Range<PhysicalAddress>) {
    let fdt = boot_info
        .memory_regions
        .iter()
        .find(|region| region.kind == MemoryRegionKind::FDT)
        .expect("no FDT region");

    let base = boot_info
        .physical_address_offset
        .checked_add(fdt.range.start)
        .unwrap() as *const u8;

    // Safety: we need to trust the bootinfo data is correct
    let slice =
        unsafe { slice::from_raw_parts(base, fdt.range.end.checked_sub(fdt.range.start).unwrap()) };
    (
        slice,
        Range::from(PhysicalAddress::new(fdt.range.start)..PhysicalAddress::new(fdt.range.end)),
    )
}
