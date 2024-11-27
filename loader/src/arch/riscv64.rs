use crate::machine_info::MachineInfo;
use crate::page_alloc::PageAllocator;
use crate::pmm::arch::{Riscv64Sv39, PAGE_SIZE};
use crate::pmm::{BumpAllocator, FrameAllocator, PhysicalAddress, VirtualAddress};
use crate::{pmm, ENABLE_KASLR};
use core::arch::naked_asm;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::ptr::{addr_of, addr_of_mut};
use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

const STACK_SIZE_PAGES: usize = 32;

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
fn start(hartid: usize, opaque: *const u8) -> ! {
    static INIT: sync::Once = sync::Once::new();

    // Disable interrupts. The kernel will re-enable interrupts
    // when it's ready to handle them
    riscv::interrupt::disable();

    INIT.call_once(|| {
        // zero out the BSS section, under QEMU we already get zeroed memory
        // but on actual hardware this might not be the case
        zero_bss();

        semihosting_logger::init(log::LevelFilter::Trace);

        let minfo = unsafe { MachineInfo::from_dtb(opaque).expect("failed to parse machine info") };
        log::info!("{minfo:?}");

        let self_regions = SelfRegions::collect(&minfo);
        log::trace!("{self_regions:?}");

        let mut frame_alloc =
            BumpAllocator::new_with_lower_bound(&minfo.memories, self_regions.read_write.end);

        let mut pmm = Riscv64Sv39::new(&mut frame_alloc, VirtualAddress::default()).unwrap();

        // Identity map the loader itself (this binary).
        //
        // we're already running in s-mode which means that once we switch on the MMU it takes effect *immediately*
        // as opposed to m-mode where it would take effect after jump tp u-mode.
        // This means we need to temporarily identity map the loader here, so we can continue executing our own code.
        // We will then unmap the loader in the kernel.
        identity_map_self(&mut pmm, &mut frame_alloc, &self_regions).unwrap();

        // Map the physical memory into kernel address space.
        //
        // This will be used by the kernel to access the page tables, BootInfo struct and maybe
        // more in the future.
        map_physical_memory(&mut pmm, &mut frame_alloc, &minfo).unwrap();
    });

    log::trace!("done...");

    loop {}
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

            start..stack_start.add(machine_info.cpus * STACK_SIZE_PAGES * PAGE_SIZE)
        };

        SelfRegions {
            executable,
            read_only,
            read_write,
        }
    }
}

fn identity_map_self(
    pmm: &mut Riscv64Sv39,
    frame_alloc: &mut dyn FrameAllocator,
    self_regions: &SelfRegions,
) -> crate::Result<Range<VirtualAddress>> {
    log::trace!(
        "Identity mapping own executable region {:?}...",
        self_regions.executable
    );
    identity_map_range(
        pmm,
        frame_alloc,
        self_regions.executable.clone(),
        pmm::Flags::READ | pmm::Flags::EXECUTE,
    )?;

    log::trace!(
        "Identity mapping own read-only region {:?}...",
        self_regions.read_only
    );
    identity_map_range(
        pmm,
        frame_alloc,
        self_regions.read_only.clone(),
        pmm::Flags::READ,
    )?;

    log::trace!(
        "Identity mapping own read-write region {:?}...",
        self_regions.read_write
    );
    identity_map_range(
        pmm,
        frame_alloc,
        self_regions.read_write.clone(),
        pmm::Flags::READ | pmm::Flags::WRITE,
    )?;

    Ok(VirtualAddress::new(self_regions.executable.start.as_raw())
        ..VirtualAddress::new(self_regions.read_write.end.as_raw()))
}

#[inline]
fn identity_map_range(
    pmm: &mut Riscv64Sv39,
    frame_alloc: &mut dyn FrameAllocator,
    phys: Range<PhysicalAddress>,
    flags: pmm::Flags,
) -> crate::Result<()> {
    let virt = VirtualAddress::new(phys.start.as_raw());
    let len = NonZeroUsize::new(phys.end.as_raw() - phys.start.as_raw()).unwrap();

    pmm.map_contiguous(frame_alloc, virt, phys.start, len, flags)
        .map_err(Into::into)
}

// TODO explain why no ASLR here
pub fn map_physical_memory(
    pmm: &mut Riscv64Sv39,
    frame_alloc: &mut dyn FrameAllocator,
    minfo: &MachineInfo,
) -> crate::Result<Range<VirtualAddress>> {
    let phys = minfo.memory_hull();
    let alignment = pmm::arch::page_size_for_level(2);

    let phys_aligned = phys.start.align_down(alignment);
    let size = phys.end.align_up(alignment).as_raw() - phys_aligned.as_raw();
    let virt = pmm::arch::KERNEL_ASPACE_BASE..pmm::arch::KERNEL_ASPACE_BASE.add(size);

    log::trace!("Mapping physical memory {phys_aligned:?}..{:?} => {virt:?}...", phys_aligned.add(size));
    pmm.map_contiguous(
        frame_alloc,
        virt.start,
        phys_aligned,
        NonZeroUsize::new(size).unwrap(),
        pmm::Flags::READ | pmm::Flags::WRITE,
    )?;

    Ok(virt)
}
