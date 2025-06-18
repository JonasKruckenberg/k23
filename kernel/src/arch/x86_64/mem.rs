// Claude generate code
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::mem::VirtualAddress;
use core::range::{Range, RangeInclusive};

pub const DEFAULT_ASID: u16 = 0;

// x86_64 canonical address layout:
// 48-bit virtual addressing means bits 47:0 are used, bits 63:48 must be sign extension
pub const VIRT_ADDR_BITS: u32 = 48;
pub const CANONICAL_ADDRESS_MASK: usize = !((1 << VIRT_ADDR_BITS) - 1);

pub const KERNEL_ASPACE_RANGE: RangeInclusive<VirtualAddress> = RangeInclusive {
    start: VirtualAddress::new(0xffff800000000000).unwrap(), // -128TB in canonical form
    end: VirtualAddress::MAX,
};

/// Virtual address where the user address space starts.
///
/// The first 2MiB are reserved for catching null pointer dereferences.
pub const USER_ASPACE_RANGE: RangeInclusive<VirtualAddress> = RangeInclusive {
    start: VirtualAddress::new(0x0000000000200000).unwrap(), // 2MB
    end: VirtualAddress::new((1 << VIRT_ADDR_BITS) - 1).unwrap(), // 256TB - 1
};

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = 12; // log2(PAGE_SIZE)

/// The number of page table entries in one table
pub const PAGE_TABLE_ENTRIES: usize = 512;
pub const PAGE_TABLE_LEVELS: usize = 4; // PML4, PDPT, PD, PT
pub const PAGE_ENTRY_SHIFT: usize = 9; // log2(PAGE_TABLE_ENTRIES)

#[cold]
pub fn init() {
    // TODO: Initialize x86_64 memory management
    // This might include setting up initial page tables, enabling features like SMEP/SMAP, etc.
}

/// Return whether the given virtual address is in the kernel address space.
pub const fn is_kernel_address(virt: VirtualAddress) -> bool {
    KERNEL_ASPACE_RANGE.start.get() <= virt.get() && virt.get() <= KERNEL_ASPACE_RANGE.end.get()
}

/// Invalidate address translation caches for the given `address_range` in the given `address_space`.
///
/// # Errors
///
/// Should return an error if the underlying operation failed and the caches could not be invalidated.
pub fn invalidate_range(_asid: u16, address_range: Range<VirtualAddress>) -> crate::Result<()> {
    // TODO: Implement INVLPG or INVPCID for x86_64
    // For now, just flush the entire TLB
    unsafe {
        let cr3_val: u64;
        core::arch::asm!("mov {}, cr3", out(reg) cr3_val);
        core::arch::asm!("mov cr3, {}", in(reg) cr3_val);
    }
    Ok(())
}

use crate::mem::frame_alloc::{Frame, FrameAllocator};
use crate::mem::{Flush, PhysicalAddress};
use alloc::vec::Vec;
use bitflags::bitflags;
use core::num::NonZeroUsize;

#[derive(Debug)]
pub struct AddressSpace {
    root_pgtable: PhysicalAddress,
    wired_frames: Vec<Frame>,
    asid: u16,
}

bitflags! {
    #[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
    pub struct PTEFlags: u64 {
        /// Page is present (valid)
        const PRESENT   = 1 << 0;
        /// Page is writable
        const WRITE     = 1 << 1;
        /// Page is accessible to user mode
        const USER      = 1 << 2;
        /// Write-through caching
        const PWT       = 1 << 3;
        /// Cache disabled
        const PCD       = 1 << 4;
        /// Page has been accessed
        const ACCESSED  = 1 << 5;
        /// Page has been written to (dirty)
        const DIRTY     = 1 << 6;
        /// Page size (0 = 4KB, 1 = large page)
        const PAGE_SIZE = 1 << 7;
        /// Global page (doesn't get flushed from TLB on CR3 reload)
        const GLOBAL    = 1 << 8;
        /// Disable execution (NX bit, requires EFER.NXE = 1)
        const NO_EXECUTE = 1 << 63;
    }
}

impl From<crate::mem::Permissions> for PTEFlags {
    fn from(flags: crate::mem::Permissions) -> Self {
        use crate::mem::Permissions;

        // Start with present bit and accessed/dirty for performance
        let mut out = Self::PRESENT | Self::ACCESSED | Self::DIRTY;

        for flag in flags {
            match flag {
                Permissions::READ => {
                    // Read is implicit with PRESENT on x86_64
                }
                Permissions::WRITE => out.insert(Self::WRITE),
                Permissions::EXECUTE => {
                    // Don't set NO_EXECUTE, allowing execution
                }
                Permissions::USER => out.insert(Self::USER),
                _ => unreachable!(),
            }
        }

        // If execute permission is NOT requested, set NO_EXECUTE
        if !flags.contains(Permissions::EXECUTE) {
            out.insert(Self::NO_EXECUTE);
        }

        out
    }
}

impl crate::mem::ArchAddressSpace for AddressSpace {
    type Flags = PTEFlags;

    fn new(asid: u16, frame_alloc: &FrameAllocator) -> crate::Result<(Self, Flush)>
    where
        Self: Sized,
    {
        // TODO: Create new page table for x86_64
        // For now, create a minimal implementation
        let root_frame = frame_alloc.alloc_one_zeroed()?;

        let this = Self {
            asid,
            root_pgtable: root_frame.addr(),
            wired_frames: alloc::vec![root_frame],
        };

        Ok((this, Flush::empty(asid)))
    }

    fn from_active(asid: u16) -> (Self, Flush)
    where
        Self: Sized,
    {
        // TODO: Get current CR3 value
        let root_pgtable = PhysicalAddress::new(0); // Placeholder

        let this = Self {
            asid,
            root_pgtable,
            wired_frames: Vec::new(),
        };

        (this, Flush::empty(asid))
    }

    unsafe fn map_contiguous(
        &mut self,
        _frame_alloc: &FrameAllocator,
        _virt: VirtualAddress,
        _phys: PhysicalAddress,
        _len: NonZeroUsize,
        _flags: Self::Flags,
        _flush: &mut Flush,
    ) -> crate::Result<()> {
        // TODO: Implement x86_64 page table mapping
        Ok(())
    }

    unsafe fn update_flags(
        &mut self,
        _virt: VirtualAddress,
        _len: NonZeroUsize,
        _new_flags: Self::Flags,
        _flush: &mut Flush,
    ) -> crate::Result<()> {
        // TODO: Implement x86_64 flag updates
        Ok(())
    }

    unsafe fn unmap(
        &mut self,
        _virt: VirtualAddress,
        _len: NonZeroUsize,
        _flush: &mut Flush,
    ) -> crate::Result<()> {
        // TODO: Implement x86_64 unmapping
        Ok(())
    }

    unsafe fn query(&mut self, _virt: VirtualAddress) -> Option<(PhysicalAddress, Self::Flags)> {
        // TODO: Implement x86_64 page table query
        None
    }

    unsafe fn activate(&self) {
        // TODO: Set CR3 to self.root_pgtable
        unsafe {
            let cr3_val = self.root_pgtable.get() as u64;
            core::arch::asm!("mov cr3, {}", in(reg) cr3_val);
        }
    }

    fn new_flush(&self) -> Flush {
        Flush::empty(self.asid)
    }
}
