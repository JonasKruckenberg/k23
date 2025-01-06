mod setjmp_longjmp;
mod start;
mod trap_handler;

use bitflags::bitflags;
use core::arch::asm;
use dtb_parser::Strings;
use fallible_iterator::FallibleIterator;
use mmu::VirtualAddress;
use riscv::sstatus::FS;
use riscv::{interrupt, scounteren, sie, sstatus};
use static_assertions::const_assert_eq;

/// Virtual address where the kernel address space starts.
///
///
pub const KERNEL_ASPACE_BASE: VirtualAddress = VirtualAddress::new(0xffffffc000000000).unwrap();
pub const KERNEL_ASPACE_SIZE: usize = 1 << mmu::arch::VIRT_ADDR_BITS;
const_assert_eq!(KERNEL_ASPACE_BASE.get(), mmu::arch::CANONICAL_ADDRESS_MASK);
const_assert_eq!(KERNEL_ASPACE_SIZE - 1, !mmu::arch::CANONICAL_ADDRESS_MASK);

/// Virtual address where the user address space starts.
///
/// The first 2MiB are reserved for catching null pointer dereferences, but this might
/// change in the future if we decide that the null-checking performed by the WASM runtime
/// is sufficiently robust.
pub const USER_ASPACE_BASE: VirtualAddress = VirtualAddress::new(0x0000000000200000).unwrap();
pub const USER_ASPACE_SIZE: usize = (1 << mmu::arch::VIRT_ADDR_BITS) - USER_ASPACE_BASE.get();

bitflags! {
    #[derive(Debug, Default, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct RiscvExtensions: u64 {
        const I = 1 << 0;
        const M = 1 << 1;
        const A = 1 << 2;
        const F = 1 << 3;
        const D = 1 << 4;
        const C = 1 << 5;
        const H = 1 << 6;
        const ZIC64B = 1 << 7;
        const ZICBOM = 1 << 8;
        const ZICBOP = 1 << 9;
        const ZICBOZ = 1 << 10;
        const ZICCAMOA = 1 << 11;
        const ZICCIF = 1 << 12;
        const ZICCLSM = 1 << 13;
        const ZICCRSE = 1 << 14;
        const ZICNTR = 1 << 15;
        const ZICSR = 1 << 16;
        const ZIFENCEI = 1 << 17;
        const ZIHINTNTL = 1 << 18;
        const ZIHINTPAUSE = 1 << 19;
        const ZIHPM = 1 << 20;
        const ZMMUL = 1 << 21;
        const ZA64RS = 1 << 22;
        const ZAAMO = 1 << 23;
        const ZALRSC = 1 << 24;
        const ZAWRS = 1 << 25;
        const ZFA = 1 << 26;
        const ZCA = 1 << 27;
        const ZCD = 1 << 28;
        const ZBA = 1 << 29;
        const ZBB = 1 << 30;
        const ZBC = 1 << 31;
        const ZBS = 1 << 32;
        const SSCCPTR = 1 << 33;
        const SSCOUNTERENW = 1 << 34;
        const SSTC = 1 << 35;
        const SSTVALA = 1 << 36;
        const SSTVECD = 1 << 37;
        const SVADU = 1 << 38;
    }
}

pub fn parse_riscv_extensions(mut strs: Strings) -> Result<RiscvExtensions, dtb_parser::Error> {
    let mut out = RiscvExtensions::empty();

    while let Some(str) = strs.next()? {
        out |= match str {
            "i" => RiscvExtensions::I,
            "m" => RiscvExtensions::M,
            "a" => RiscvExtensions::A,
            "f" => RiscvExtensions::F,
            "d" => RiscvExtensions::D,
            "c" => RiscvExtensions::C,
            "h" => RiscvExtensions::H,
            "zic64b" => RiscvExtensions::ZIC64B,
            "zicbom" => RiscvExtensions::ZICBOM,
            "zicbop" => RiscvExtensions::ZICBOP,
            "zicboz" => RiscvExtensions::ZICBOZ,
            "ziccamoa" => RiscvExtensions::ZICCAMOA,
            "ziccif" => RiscvExtensions::ZICCIF,
            "zicclsm" => RiscvExtensions::ZICCLSM,
            "ziccrse" => RiscvExtensions::ZICCRSE,
            "zicntr" => RiscvExtensions::ZICNTR,
            "zicsr" => RiscvExtensions::ZICSR,
            "zifencei" => RiscvExtensions::ZIFENCEI,
            "zihintntl" => RiscvExtensions::ZIHINTNTL,
            "zihintpause" => RiscvExtensions::ZIHINTPAUSE,
            "zihpm" => RiscvExtensions::ZIHPM,
            "zmmul" => RiscvExtensions::ZMMUL,
            "za64rs" => RiscvExtensions::ZA64RS,
            "zaamo" => RiscvExtensions::ZAAMO,
            "zalrsc" => RiscvExtensions::ZALRSC,
            "zawrs" => RiscvExtensions::ZAWRS,
            "zfa" => RiscvExtensions::ZFA,
            "zca" => RiscvExtensions::ZCA,
            "zcd" => RiscvExtensions::ZCD,
            "zba" => RiscvExtensions::ZBA,
            "zbb" => RiscvExtensions::ZBB,
            "zbc" => RiscvExtensions::ZBC,
            "zbs" => RiscvExtensions::ZBS,
            "ssccptr" => RiscvExtensions::SSCCPTR,
            "sscounterenw" => RiscvExtensions::SSCOUNTERENW,
            "sstc" => RiscvExtensions::SSTC,
            "sstvala" => RiscvExtensions::SSTVALA,
            "sstvecd" => RiscvExtensions::SSTVECD,
            "svadu" => RiscvExtensions::SVADU,
            _ => return Err(dtb_parser::Error::InvalidToken(0)), // TODO better error type
        }
    }

    Ok(out)
}

pub fn init() {
    let supported = riscv::sbi::supported_extensions().unwrap();
    log::trace!("Supported SBI extensions: {supported:?}");

    log::trace!("BOOT STACK {:?}", start::BOOT_STACK.0.as_ptr_range())

    // TODO riscv64_mmu_early_init
    //      - figure out ASID bits
    //      -  Zero the bottom of the kernel page table to remove any left over boot mappings.
    // TODO riscv64_mmu_early_init_percpu
}

pub fn per_hart_init() {
    unsafe {
        // Initialize the trap handler
        trap_handler::init();

        // Enable interrupts
        interrupt::enable();

        // Enable supervisor timer and external interrupts
        sie::set_stie();
        sie::set_seie();

        // enable counters
        scounteren::set_cy();
        scounteren::set_tm();
        scounteren::set_ir();

        // Set the FPU state to initial
        sstatus::set_fs(FS::Initial);
    }
}

/// Return whether the given virtual address is in the kernel address space.
pub const fn is_kernel_address(virt: VirtualAddress) -> bool {
    virt.get() >= KERNEL_ASPACE_BASE.get()
        && virt.checked_sub_addr(KERNEL_ASPACE_BASE).unwrap() < KERNEL_ASPACE_SIZE
}

/// Set the thread pointer on the calling hart to the given address.
pub fn set_thread_ptr(addr: VirtualAddress) {
    unsafe {
        asm!("mv tp, {addr}", addr = in(reg) addr.get());
    }
}

/// Suspend the calling hart until an interrupt is received.
pub fn wait_for_interrupt() {
    unsafe { asm!("wfi") }
}
