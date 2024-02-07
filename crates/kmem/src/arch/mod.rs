use crate::{PhysicalAddress, VirtualAddress};
use core::mem;
use core::ops::Range;

cfg_if::cfg_if! {
    if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
        pub use riscv64::*;
    }
    else {
        compile_error!("Unsupported architecture");
    }
}

pub trait Arch {
    const PAGE_SIZE: usize;
    const VIRT_ADDR_BITS: u32;
    const PAGE_LEVELS: usize;

    const ENTRY_FLAG_VALID: usize;
    const ENTRY_FLAG_READ: usize;
    const ENTRY_FLAG_WRITE: usize;
    const ENTRY_FLAG_EXECUTE: usize;
    const ENTRY_FLAG_USER: usize;

    /// How wide is each virtual address' physical page number (PPN) in bits?
    const ADDR_PPN_BITS: usize;
    /// How wide is the virtual address' page offset in bits?
    const ADDR_OFFSET_BITS: usize;
    const ADDR_PPN_MASK: usize = (1 << Self::ADDR_PPN_BITS) - 1;
    const ADDR_OFFSET_MASK: usize = (1 << Self::ADDR_OFFSET_BITS) - 1;

    const ENTRY_FLAGS_MASK: usize;
    const ENTRY_ADDR_SHIFT: usize;

    /// The offset from physical memory at which the kernel will be mapped.
    const PHYS_OFFSET: usize;

    unsafe fn active_table(address_space: usize) -> PhysicalAddress;

    unsafe fn activate_table(table: PhysicalAddress, address_space: usize);

    /// Invalidate all address translation caches across all address spaces
    fn invalidate_all() -> crate::Result<()>;

    /// Invalidate address translation caches for the given `address_range` in the given `address_space`
    fn invalidate_range(
        address_space: usize,
        address_range: Range<VirtualAddress>,
    ) -> crate::Result<()>;

    unsafe fn phys_to_virt(phys: PhysicalAddress) -> VirtualAddress {
        match phys.as_raw().checked_add(Self::PHYS_OFFSET) {
            Some(some) => VirtualAddress::new(some),
            None => panic!("phys_to_virt({:#x}) overflow", phys.as_raw()),
        }
    }

    // fn canonicalize_virt(virt: VirtualAddress) -> VirtualAddress {
    //     let shift = mem::size_of::<usize>() as u32 * 8 - Self::VIRT_ADDR_BITS;
    //
    //     VirtualAddress(virt.0.wrapping_shl(shift).wrapping_shr(shift))
    // }
}
