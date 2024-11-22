#![no_std]
#![no_main]
#![allow(incomplete_features)]
#![feature(generic_const_exprs)]
#![feature(naked_functions)]
#![feature(maybe_uninit_slice)]
#![feature(int_roundings)]

mod arch;
mod boot_info;
mod error;
mod kernel;
mod machine_info;
mod page_alloc;
mod vm;

pub const STACK_SIZE_PAGES: usize = 32;
pub const LOG_LEVEL: log::Level = log::Level::Trace;
pub const ENABLE_KASLR: bool = true;

use crate::kernel::{parse_inlined_kernel, Kernel};
use crate::machine_info::MachineInfo;
use crate::vm::KernelAddressSpace;
use core::ops::Range;
use core::ptr::addr_of;
use core::{ptr, slice};
use cfg_if::cfg_if;
use error::Error;
use pmm::{BumpAllocator, FrameAllocator, PhysicalAddress, VirtualAddress};
use sync::Mutex;

pub type Result<T> = core::result::Result<T, Error>;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let location = info.location().map(|l| l.file()).unwrap_or("<unknown>");
    let line = info.location().map(|l| l.line()).unwrap_or(0);
    let col = info.location().map(|l| l.column()).unwrap_or(0);

    log::error!("hart panicked at {location}:{line}:{col}: \n{}", info.message());

    cfg_if! {
        if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
            riscv::abort();
        } else {
            loop {}
        }
    }
}

fn main<A>(hartid: usize, pmm_arch: &'static Mutex<A>, minfo: &'static MachineInfo) -> !
where
    A: pmm::Arch,
    [(); A::PAGE_TABLE_ENTRIES / 2]: Sized,
{
    static INIT: sync::OnceLock<(KernelAddressSpace, PhysicalAddress)> = sync::OnceLock::new();

    let (kernel_aspace, boot_info) = INIT
        .get_or_try_init(|| -> Result<(KernelAddressSpace, PhysicalAddress)> {
            log::info!("welcome to k23 v{}", env!("CARGO_PKG_VERSION"));

            let loader_regions = LoaderRegions::new::<A>(minfo);

            let mut frame_alloc: BumpAllocator<A> = unsafe {
                BumpAllocator::new_with_lower_bound(
                    &minfo.memories,
                    loader_regions.read_write.end,
                    VirtualAddress::default(), // while we haven't activated the virtual memory we have not offset
                )
            };

            // Move the device tree blob from wherever random place the previous bootloader put it
            // into a properly allocated place so we don't accidentally override it
            let fdt = alloc_and_copy_fdt(minfo, &mut frame_alloc)?;

            // Parse the inlined kernel ELF file
            let kernel = parse_inlined_kernel()?;

            let mut pmm_arch = pmm_arch.lock();
            // Initialize the kernel address space
            let kernel_aspace =
                KernelAddressSpace::new(&mut pmm_arch, &mut frame_alloc, &kernel, loader_regions, minfo)?;

            // Set up the BootInfo struct that we will pass on to the kernel
            let boot_info =
                boot_info::init_boot_info(&mut frame_alloc, hartid, &kernel_aspace, &kernel, fdt)?;

            Ok((kernel_aspace, boot_info))
        })
        .expect("failed global initialization");

    // SAFETY: This will invalidate all pointers and references that don't point to the loader stack
    // (the FDT slice and importantly the frame allocator) so care has to be taken to either
    // not access these anymore (which should be easy, this is one of the last steps we perform before hading off
    // to the kernel) or to map them into virtual memory first!
    unsafe {
        log::debug!("[HART {hartid}] Activating kernel address space...");
        kernel_aspace.activate::<A>();
    }

    log::debug!("[HART {hartid}] Initializing TLS region...");
    kernel_aspace.init_tls_region_for_hart(hartid);

    // Safety: We essentially jump to arbitrary memory here. But we have no choice
    // other than to rely on `KernelAddressSpace::entry_virt` being correct.
    unsafe {
        arch::handoff_to_kernel(
            hartid,
            kernel_aspace.entry_virt(),
            kernel_aspace.stack_region_for_hart(hartid),
            kernel_aspace
                .tls_region_for_hart(hartid)
                .unwrap_or_default()
                .start,
            // TODO make fn
            kernel_aspace.physmap().start.add(boot_info.as_raw()),
        )
    }
}

pub fn alloc_and_copy_fdt<A>(
    machine_info: &MachineInfo,
    alloc: &mut BumpAllocator<A>,
) -> Result<PhysicalAddress>
where
    A: pmm::Arch,
{
    let frames = machine_info.fdt.len().div_ceil(A::PAGE_SIZE);
    let base = alloc.allocate_frames(frames)?;

    unsafe {
        let dst = slice::from_raw_parts_mut(base.as_raw() as *mut u8, machine_info.fdt.len());

        ptr::copy_nonoverlapping(machine_info.fdt.as_ptr(), dst.as_mut_ptr(), dst.len());
    }

    Ok(base)
}

#[derive(Debug)]
pub struct LoaderRegions {
    pub executable: Range<PhysicalAddress>,
    pub read_only: Range<PhysicalAddress>,
    pub read_write: Range<PhysicalAddress>,
}

impl LoaderRegions {
    #[must_use]
    pub fn new<A>(machine_info: &MachineInfo) -> Self
    where
        A: pmm::Arch,
    {
        extern "C" {
            static __text_start: u8;
            static __text_end: u8;
            static __rodata_start: u8;
            static __rodata_end: u8;
            static __bss_start: u8;
            static __stack_start: u8;
        }

        let executable: Range<PhysicalAddress> = {
            PhysicalAddress::new(addr_of!(__text_start) as usize)
                ..PhysicalAddress::new(addr_of!(__text_end) as usize)
        };

        let read_only: Range<PhysicalAddress> = {
            PhysicalAddress::new(addr_of!(__rodata_start) as usize)
                ..PhysicalAddress::new(addr_of!(__rodata_end) as usize)
        };

        let read_write: Range<PhysicalAddress> = {
            let start = PhysicalAddress::new(addr_of!(__bss_start) as usize);
            let stack_start = PhysicalAddress::new(addr_of!(__stack_start) as usize);

            start..stack_start.add(machine_info.cpus * STACK_SIZE_PAGES * A::PAGE_SIZE)
        };

        LoaderRegions {
            executable,
            read_only,
            read_write,
        }
    }
}
