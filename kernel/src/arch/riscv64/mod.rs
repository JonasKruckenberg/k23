mod setjmp_longjmp;
pub mod trap_handler;
pub mod vm;

use core::arch::asm;
use mmu::{AddressRangeExt, VirtualAddress};
use riscv::sstatus::FS;
use riscv::{interrupt, sie, sstatus};
use static_assertions::{const_assert, const_assert_eq};

pub use setjmp_longjmp::{longjmp, setjmp, JumpBuf};

/// Virtual address where the kernel address space starts.
///
///
pub const KERNEL_ASPACE_BASE: VirtualAddress = VirtualAddress::new(0xffffffc000000000).unwrap();
pub const KERNEL_ASPACE_SIZE: usize = (1 << mmu::arch::VIRT_ADDR_BITS);
const_assert_eq!(KERNEL_ASPACE_BASE.get(), mmu::arch::CANONICAL_ADDRESS_MASK);
const_assert_eq!(KERNEL_ASPACE_SIZE - 1, !mmu::arch::CANONICAL_ADDRESS_MASK);

/// Virtual address where the user address space starts.
///
/// The first 2MiB are reserved for catching null pointer dereferences, but this might
/// change in the future if we decide that the null-checking performed by the WASM runtime
/// is sufficiently robust.
pub const USER_ASPACE_BASE: VirtualAddress = VirtualAddress::new(0x0000000000200000).unwrap();
pub const USER_ASPACE_SIZE: usize = (1 << mmu::arch::VIRT_ADDR_BITS) - USER_ASPACE_BASE.get();

/// Return whether the given virtual address is in the kernel address space.
pub const fn is_kernel_address(virt: VirtualAddress) -> bool {
    virt.get() >= KERNEL_ASPACE_BASE.get()
        && virt.checked_sub_addr(KERNEL_ASPACE_BASE).unwrap() < KERNEL_ASPACE_SIZE
}

/// Suspend the calling hart until an interrupt is received.
pub fn wait_for_interrupt() {
    unsafe { asm!("wfi") }
}

/// Early architecture-specific, per-hart initialization.
pub fn hart_init_early() {}

/// Late architecture-specific, per-hart initialization.
pub fn hart_init_late() {
    unsafe {
        // Enable interrupts
        interrupt::enable();
        // Enable supervisor timer interrupts
        sie::set_stie();
        // Enable FPU
        sstatus::set_fs(FS::Initial);
    }
}
