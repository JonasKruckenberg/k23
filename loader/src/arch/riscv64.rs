use crate::boot_info::init_boot_info;
use crate::kernel::{parse_kernel, INLINED_KERNEL_BYTES};
use crate::machine_info::MachineInfo;
use crate::page_alloc::PageAllocator;
use crate::vm::{init_kernel_aspace, KernelAddressSpace};
use crate::{ENABLE_KASLR, LOG_LEVEL};
use arrayvec::ArrayVec;
use core::alloc::Layout;
use core::arch::{asm, naked_asm};
use core::num::NonZeroUsize;
use core::ops::Range;
use core::ptr::{addr_of, addr_of_mut};
use core::{cmp, ptr, slice};
use mmu::arch::PAGE_SIZE;
use mmu::frame_alloc::BootstrapAllocator;
use mmu::{arch, AddressRangeExt, AddressSpace, Error};
use mmu::{
    frame_alloc::{BuddyAllocator, FrameAllocator},
    Flush, PhysicalAddress, VirtualAddress,
};
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

const STACK_SIZE_PAGES: usize = 32;
const KERNEL_ASPACE_BASE: VirtualAddress = VirtualAddress::new(0xffffffc000000000).unwrap();
const KERNEL_ASID: usize = 0;

/// The main entry point for the loader
///
/// This sets up the global and stack pointer, as well as filling the stack with a known debug pattern
/// and then - as fast as possible - jumps to Rust.
#[link_section = ".text.start"]
#[no_mangle]
#[naked]
unsafe extern "C" fn _start() -> ! {
    naked_asm!(
        ".option push",
        ".option norelax",
        "la		gp, __global_pointer$",
        ".option pop",
        // read boot time stamp as early as possible
        "rdtime a2",

        "la     sp, __stack_start", // set the stack pointer to the bottom of the stack
        "li     t0, {stack_size}", // load the stack size
        "addi   t1, a0, 1", // add one to the hart id so that we add at least one stack size (stack grows from the top downwards)
        "mul    t1, t0, t1", // multiply the stack size by the hart id to get the offset
        "add    sp, sp, t1", // add the offset from sp to get the harts stack pointer

        "call {fillstack}",

        "jal zero, {start_rust}",   // jump into Rust

        stack_size = const STACK_SIZE_PAGES * PAGE_SIZE,

        fillstack = sym fillstack,
        start_rust = sym start,
    )
}

/// Fill the stack with a canary pattern (0xACE0BACE) so that we can identify unused stack memory
/// in dumps & calculate stack usage. This is also really great (don't ask my why I know this) to identify
/// when we tried executing stack memory.
///
/// # Safety
///
/// expects the bottom of `stack_size` in `t0` and the top of stack in `sp`
#[naked]
unsafe extern "C" fn fillstack() {
    naked_asm!(
        "li          t1, 0xACE0BACE",
        "sub         t0, sp, t0", // subtract stack_size from sp to get the bottom of stack
        "100:",
        "sw          t1, 0(t0)",
        "addi        t0, t0, 8",
        "bltu        t0, sp, 100b",
        "ret",
    )
}

/// Architecture specific startup code
fn start(hartid: usize, opaque: *const u8, boot_ticks: u64) -> ! {
    static INIT: sync::OnceLock<(KernelAddressSpace, VirtualAddress)> = sync::OnceLock::new();

    // Disable interrupts. The kernel will re-enable interrupts
    // when it's ready to handle them
    riscv::interrupt::disable();

    let (kernel_aspace, boot_info) = INIT
        .get_or_try_init(|| -> crate::Result<_> {
            // zero out the BSS section, under QEMU we already get zeroed memory
            // but on actual hardware this might not be the case
            zero_bss();

            semihosting_logger::init(LOG_LEVEL);

            let minfo =
                unsafe { MachineInfo::from_dtb(opaque).expect("failed to parse machine info") };
            log::debug!("\n{minfo}");

            let self_regions = SelfRegions::collect(&minfo);
            log::trace!("{self_regions:?}");

            let allocatable_memories = allocatable_memory_regions(&minfo, &self_regions);
            let mut frame_alloc = BootstrapAllocator::new(&allocatable_memories);

            let mut page_alloc = if ENABLE_KASLR {
                PageAllocator::new(ChaCha20Rng::from_seed(
                    minfo.rng_seed.unwrap()[0..32].try_into().unwrap(),
                ))
            } else {
                PageAllocator::new_no_kaslr()
            };

            let (mut aspace, mut flush) =
                AddressSpace::new(&mut frame_alloc, KERNEL_ASID, VirtualAddress::default())?;

            let fdt_phys = allocate_and_copy(&mut frame_alloc, minfo.fdt)?;
            let kernel_phys = allocate_and_copy(&mut frame_alloc, &INLINED_KERNEL_BYTES.0)?;

            // Identity map the loader itself (this binary).
            //
            // we're already running in s-mode which means that once we switch on the MMU it takes effect *immediately*
            // as opposed to m-mode where it would take effect after jump tp u-mode.
            // This means we need to temporarily identity map the loader here, so we can continue executing our own code.
            // We will then unmap the loader in the kernel.
            identity_map_self(&mut aspace, &mut frame_alloc, &self_regions, &mut flush)?;

            // Map the physical memory into kernel address space.
            //
            // This will be used by the kernel to access the page tables, BootInfo struct and maybe
            // more in the future.
            let (phys_off, phys_map) = map_physical_memory(
                &mut aspace,
                &mut frame_alloc,
                &mut page_alloc,
                &minfo,
                &mut flush,
            )?;

            // Activate the MMU with the address space we have built so far.
            // the rest of the address space setup will happen in virtual memory (mostly so that we
            // can correctly apply relocations without having to do expensive virt to phys queries)
            unsafe {
                log::trace!("activating MMU...");
                flush.ignore();
                aspace.activate();
                log::trace!("activated.");
            }
            frame_alloc.set_phys_offset(phys_off);

            // The kernel elf file is inlined into the loader executable as part of the build setup
            // which means we just need to parse it here.
            let kernel = parse_kernel(unsafe {
                slice::from_ptr_range(
                    kernel_phys
                        .clone()
                        .checked_add(phys_off.get())
                        .unwrap()
                        .as_ptr_range(),
                )
            })?;
            // print the elf sections for debugging purposes
            log::debug!("\n{kernel}");

            // Reconstruct the aspace with the new physical memory mapping offset since we're in virtual
            // memory mode now.
            let (aspace, mut flush) = AddressSpace::from_active(KERNEL_ASID, phys_off);

            let kernel_aspace = init_kernel_aspace(
                aspace,
                &mut flush,
                &mut frame_alloc,
                &mut page_alloc,
                &kernel,
                &minfo,
            )?;
            // log::trace!("\n{}", kernel_aspace.aspace);

            let boot_info = init_boot_info(
                frame_alloc,
                hartid,
                minfo.hart_mask,
                &kernel_aspace,
                phys_off,
                phys_map,
                fdt_phys,
                self_regions.executable.start..self_regions.read_write.end,
                kernel_phys,
                boot_ticks
            )?;

            Ok((
                kernel_aspace,
                VirtualAddress::new(boot_info as usize).unwrap(),
            ))
        })
        .unwrap();

    kernel_aspace.init_tls_region_for_hart(hartid);
    unsafe {
        kernel_aspace.activate();
        handoff_to_kernel(
            hartid,
            kernel_aspace.kernel_entry(),
            kernel_aspace.stack_region_for_hart(hartid),
            kernel_aspace
                .tls_region_for_hart(hartid)
                .unwrap_or_default()
                .start,
            *boot_info,
        );
    }
}

fn zero_bss() {
    extern "C" {
        static mut __bss_start: u64;
        static mut __bss_end: u64;
    }

    unsafe {
        // Zero BSS section
        let mut ptr = addr_of_mut!(__bss_start);
        let end = addr_of_mut!(__bss_end);
        while ptr < end {
            ptr.write_volatile(0);
            ptr = ptr.offset(1);
        }
    }
}

/// Information about our own memory regions.
/// Used for identity mapping and calculating available physical memory.
#[derive(Debug)]
pub struct SelfRegions {
    pub executable: Range<PhysicalAddress>,
    pub read_only: Range<PhysicalAddress>,
    pub read_write: Range<PhysicalAddress>,
}

impl SelfRegions {
    #[must_use]
    pub fn collect(machine_info: &MachineInfo) -> Self {
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

            start
                ..stack_start
                    .checked_add(machine_info.cpus * STACK_SIZE_PAGES * PAGE_SIZE)
                    .unwrap()
        };

        SelfRegions {
            executable,
            read_only,
            read_write,
        }
    }
}

fn identity_map_self(
    aspace: &mut AddressSpace,
    frame_alloc: &mut dyn FrameAllocator,
    self_regions: &SelfRegions,
    flush: &mut Flush,
) -> crate::Result<()> {
    log::trace!(
        "Identity mapping loader executable region {:?}...",
        self_regions.executable
    );
    identity_map_range(
        aspace,
        frame_alloc,
        self_regions.executable.clone(),
        mmu::Flags::READ | mmu::Flags::EXECUTE,
        flush,
    )?;

    log::trace!(
        "Identity mapping loader read-only region {:?}...",
        self_regions.read_only
    );
    identity_map_range(
        aspace,
        frame_alloc,
        self_regions.read_only.clone(),
        mmu::Flags::READ,
        flush,
    )?;

    log::trace!(
        "Identity mapping loader read-write region {:?}...",
        self_regions.read_write
    );
    identity_map_range(
        aspace,
        frame_alloc,
        self_regions.read_write.clone(),
        mmu::Flags::READ | mmu::Flags::WRITE,
        flush,
    )?;

    Ok(())
}

#[inline]
fn identity_map_range(
    aspace: &mut AddressSpace,
    frame_alloc: &mut dyn FrameAllocator,
    phys: Range<PhysicalAddress>,
    flags: mmu::Flags,
    flush: &mut Flush,
) -> crate::Result<()> {
    let virt = VirtualAddress::new(phys.start.get()).unwrap();
    let len = NonZeroUsize::new(phys.size()).unwrap();

    aspace
        .map_contiguous(frame_alloc, virt, phys.start, len, flags, flush)
        .map_err(Into::into)
}

// TODO explain why no ASLR here
pub fn map_physical_memory(
    aspace: &mut AddressSpace,
    frame_alloc: &mut dyn FrameAllocator,
    page_alloc: &mut PageAllocator,
    minfo: &MachineInfo,
    flush: &mut Flush,
) -> crate::Result<(VirtualAddress, Range<VirtualAddress>)> {
    let alignment = arch::page_size_for_level(2);

    let phys = minfo.memory_hull().checked_align_out(alignment).unwrap();
    let virt = VirtualAddress::from_phys(phys.start, KERNEL_ASPACE_BASE).unwrap()
        ..VirtualAddress::from_phys(phys.end, KERNEL_ASPACE_BASE).unwrap();

    debug_assert!(phys.start.is_aligned_to(alignment) && phys.end.is_aligned_to(alignment));
    debug_assert!(virt.start.is_aligned_to(alignment) && virt.end.is_aligned_to(alignment));
    debug_assert_eq!(phys.size(), virt.size());

    log::trace!("Mapping physical memory {phys:?} => {virt:?}...",);
    aspace.map_contiguous(
        frame_alloc,
        virt.start,
        phys.start,
        NonZeroUsize::new(phys.size()).unwrap(),
        mmu::Flags::READ | mmu::Flags::WRITE,
        flush,
    )?;

    // exclude the physical memory map region from page allocation
    page_alloc.reserve(virt.start, phys.size());

    Ok((KERNEL_ASPACE_BASE, virt))
}

/// Moves the FDT from wherever the previous bootloader placed it into a properly allocated place,
/// so we don't accidentally override it
///
/// # Errors
///
/// Returns an error if allocation fails.
pub fn allocate_and_copy(
    frame_alloc: &mut dyn FrameAllocator,
    src: &[u8],
) -> crate::Result<Range<PhysicalAddress>> {
    let layout = Layout::from_size_align(src.len(), PAGE_SIZE).unwrap();
    let base = frame_alloc
        .allocate_contiguous(layout)
        .ok_or(Error::OutOfMemory)?;

    unsafe {
        let dst = slice::from_raw_parts_mut(base.as_mut_ptr(), src.len());

        ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
    }

    Ok(base..base.checked_add(layout.size()).unwrap())
}

fn allocatable_memory_regions(
    minfo: &MachineInfo,
    self_regions: &SelfRegions,
) -> ArrayVec<Range<PhysicalAddress>, 16> {
    let mut out = ArrayVec::new();
    let to_exclude = self_regions.executable.start..self_regions.read_write.end;

    for mut region in minfo.memories.clone() {
        if to_exclude.contains(&region.start) && to_exclude.contains(&region.end) {
            // remove region
            continue;
        } else if region.contains(&to_exclude.start) && region.contains(&to_exclude.end) {
            out.push(region.start..to_exclude.start);
            out.push(to_exclude.end..region.end);
        } else if to_exclude.contains(&region.start) {
            region.start = to_exclude.end;
            out.push(region);
        } else if to_exclude.contains(&region.end) {
            region.end = to_exclude.start;
            out.push(region);
        } else {
            out.push(region);
        }
    }

    out
}

pub unsafe fn handoff_to_kernel(
    hartid: usize,
    entry: VirtualAddress,
    stack: Range<VirtualAddress>,
    thread_ptr: VirtualAddress,
    boot_info: VirtualAddress,
) -> ! {
    let stack_ptr = stack.end;
    let stack_size = stack_ptr.checked_sub_addr(stack.start).unwrap();

    log::debug!("Hart {hartid} Jumping to kernel ({entry:?})...");
    log::trace!("Hart {hartid} Kernel arguments: sp = {stack_ptr:?}, tp = {thread_ptr:?}, a0 = {hartid}, a1 = {boot_info:?}");

    asm!(
    "mv  sp, {stack_ptr}", // Set the kernel stack ptr

    //  fill stack with canary pattern
    "call {fillstack}",

    "mv tp, {thread_ptr}",  // Set thread ptr
    "mv ra, zero", // Reset return address

    "jalr zero, {func}", // Jump to kernel

    // We should never ever reach this code, but if we do just spin indefinitely
    "1:",
    "   wfi",
    "   j 1b",
    in("a0") hartid,
    in("a1") boot_info.get(),
    in("t0") stack_size,
    stack_ptr = in(reg) stack_ptr.get(),
    thread_ptr = in(reg) thread_ptr.get(),
    func = in(reg) entry.get(),
    fillstack = sym fillstack,
    options(noreturn)
    )
}
