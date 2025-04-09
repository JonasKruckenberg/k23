// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::GlobalInitResult;
use crate::error::Error;
use crate::frame_alloc::FrameAllocator;
use crate::machine_info::MachineInfo;
use crate::mapping::Flags;
use bitflags::bitflags;
use core::arch::{asm, naked_asm};
use core::fmt;
use core::num::NonZero;
use core::ptr::NonNull;
use riscv::satp;

pub const DEFAULT_ASID: u16 = 0;
pub const KERNEL_ASPACE_BASE: usize = 0xffffffc000000000;
pub const PAGE_SIZE: usize = 4096;
/// The number of page table entries in one table
pub const PAGE_TABLE_ENTRIES: usize = 512;
pub const PAGE_TABLE_LEVELS: usize = 3; // L0, L1, L2 Sv39
pub const VIRT_ADDR_BITS: u32 = 38;

pub const PAGE_SHIFT: usize = (PAGE_SIZE - 1).count_ones() as usize;
pub const PAGE_ENTRY_SHIFT: usize = (PAGE_TABLE_ENTRIES - 1).count_ones() as usize;

/// On `RiscV` targets the page table entry's physical address bits are shifted 2 bits to the right.
const PTE_PPN_SHIFT: usize = 2;

/// Entry point for the initializing hart, this will set up the CPU environment for Rust and then
/// transfer control to [`crate::main`].
///
/// For the entry point of all secondary harts see [`_start_secondary`].
#[unsafe(link_section = ".text.start")]
#[unsafe(no_mangle)]
#[naked]
unsafe extern "C" fn _start() -> ! {
    // Safety: inline assembly
    unsafe {
        naked_asm! {
            // FIXME this is a workaround for bug in rustc/llvm
            //  https://github.com/rust-lang/rust/issues/80608#issuecomment-1094267279
            ".attribute arch, \"rv64gc\"",

            // read boot time stamp as early as possible
            "rdtime a2",

            // Clear return address and frame pointer
            "mv     ra, zero",
            "mv     s0, zero",

            // Clear the gp register in case anything tries to use it.
            "mv     gp, zero",

            // Mask all interrupts in case the previous stage left them on.
            "csrc   sstatus, 1 << 1",
            "csrw   sie, zero",

            // Reset the trap vector in case the previous stage left one installed.
            "csrw   stvec, zero",

            // Disable the MMU in case it was left on.
            "csrw   satp, zero",

            // Setup the stack pointer
            "la     t0, __stack_start", // set the stack pointer to the bottom of the stack
            "li     t1, {stack_size}",  // load the stack size
            "mul    sp, a0, t1",        // multiply the stack size by the hart id to get the relative stack bottom offset
            "add    t0, t0, sp",        // add the relative stack bottom offset to the absolute stack region offset to get
                                        // the absolute stack bottom
            "add    sp, t0, t1",        // add one stack size again to get to the top of the stack. This is our final stack pointer.

            // fill stack with canary pattern
            // $sp is set to stack top above, $t0 as well
            "call   {fill_stack}",

            // Clear .bss.  The linker script ensures these are aligned to 16 bytes.
            "lla    a3, __bss_zero_start",
            "lla    a4, __bss_end",
            "0:",
            "   sd      zero, (a3)",
            "   sd      zero, 8(a3)",
            "   add     a3, a3, 16",
            "   blt     a3, a4, 0b",

            // Call the rust entry point
            "call {start_rust}",

            // Loop forever.
            // `start_rust` should never return, but in case it does prevent the hart from executing
            // random code
            "2:",
            "   wfi",
            "   j 2b",

            stack_size = const crate::STACK_SIZE,
            start_rust = sym crate::main,
            fill_stack = sym fill_stack
        }
    }
}

/// Entry point for all secondary harts, this is essentially the same as [`_start`] but it doesn't
/// attempt to zero out the BSS.
///
/// It will however transfer control to the common [`crate::main`] routine.
#[naked]
unsafe extern "C" fn _start_secondary() -> ! {
    // Safety: inline assembly
    unsafe {
        naked_asm! {
            // read boot time stamp as early as possible
            "rdtime a2",

            // Clear return address and frame pointer
            "mv     ra, zero",
            "mv     s0, zero",

            // Clear the gp register in case anything tries to use it.
            "mv     gp, zero",

            // Mask all interrupts in case the previous stage left them on.
            "csrc   sstatus, 1 << 1",
            "csrw   sie, zero",

            // Reset the trap vector in case the previous stage left one installed.
            "csrw   stvec, zero",

            // Disable the MMU in case it was left on.
            "csrw   satp, zero",

            // Setup the stack pointer
            "la     t0, __stack_start", // set the stack pointer to the bottom of the stack
            "li     t1, {stack_size}",  // load the stack size
            "mul    sp, a0, t1",        // multiply the stack size by the hart id to get the relative stack bottom offset
            "add    t0, t0, sp",        // add the relative stack bottom offset to the absolute stack region offset to get
                                        // the absolute stack bottom
            "add    sp, t0, t1",        // add one stack size again to get to the top of the stack. This is our final stack pointer.

            // fill stack with canary pattern
            // $sp is set to stack top above, $t0 as well
            "call   {fill_stack}",

            // Call the rust entry point
            "call {start_rust}",

            // Loop forever.
            // `start_rust` should never return, but in case it does prevent the hart from executing
            // random code
            "2:",
            "   wfi",
            "   j 2b",

            stack_size = const crate::STACK_SIZE,
            start_rust = sym crate::main,
            fill_stack = sym fill_stack
        }
    }
}

/// Fill the stack with a canary pattern (0xACE0BACE) so that we can identify unused stack memory
/// in dumps & calculate stack usage. This is also really great (don't ask my why I know this) to identify
/// when we tried executing stack memory.
///
/// # Safety
///
/// expects the bottom of the stack in `t0` and the top of stack in `sp`
#[naked]
unsafe extern "C" fn fill_stack() {
    // Safety: inline assembly
    unsafe {
        naked_asm! {
            // Fill the stack with a canary pattern (0xACE0BACE) so that we can identify unused stack memory
            // in dumps & calculate stack usage. This is also really great (don't ask my why I know this) to identify
            // when we tried executing stack memory.
            "li     t1, 0xACE0BACE",
            "1:",
            "   sw          t1, 0(t0)",     // write the canary as u64
            "   addi        t0, t0, 8",     // move to the next u64
            "   bltu        t0, sp, 1b",    // loop until we reach the top of the stack
            "ret"
        }
    }
}

/// This will hand off control over this CPU to the kernel. This is the last function executed in
/// the loader and will never return.
pub unsafe fn handoff_to_kernel(hartid: usize, boot_ticks: u64, init: &GlobalInitResult) -> ! {
    let stack = init.stacks_alloc.region_for_cpu(hartid);
    let tls = init
        .maybe_tls_alloc
        .as_ref()
        .map(|tls| tls.region_for_hart(hartid))
        .unwrap_or_default();

    log::debug!("Hart {hartid} Jumping to kernel...");
    log::trace!(
        "Hart {hartid} entry: {:#x}, arguments: a0={hartid} a1={:?} stack={stack:#x?} tls={tls:#x?}",
        init.kernel_entry,
        init.boot_info
    );

    // Synchronize all harts before jumping to the kernel.
    // Technically this isn't really necessary, but debugging output gets horribly mangled if we don't
    // and that's terrible for this critical transition
    init.barrier.wait();

    // Safety: inline assembly
    unsafe {
        riscv::sstatus::set_sum();

        asm! {
            "mv  sp, {stack_top}", // Set the kernel stack ptr
            "mv  tp, {tls_start}", // Set the kernel thread ptr

            // fill stack with canary pattern
            // $sp is set to stack top above, $t0 is set to stack bottom by the asm args below
            "call {fill_stack}",

            "mv ra, zero", // Reset return address

            "jalr zero, {kernel_entry}",

            // Loop forever.
            // The kernel should never return, but in case it does prevent the hart from executing
            // random code
            "1:",
            "   wfi",
            "   j 1b",
            in("a0") hartid,
            in("a1") init.boot_info,
            in("a2") boot_ticks,
            in("t0") stack.start,
            stack_top = in(reg) stack.end,
            tls_start = in(reg) tls.start,
            kernel_entry = in(reg) init.kernel_entry,
            fill_stack = sym fill_stack,
            options(noreturn)
        }
    }
}

/// Start all secondary harts on the system as reported by [`MachineInfo`].
pub fn start_secondary_harts(boot_hart: usize, minfo: &MachineInfo) -> crate::Result<()> {
    let start = minfo.hart_mask.trailing_zeros() as usize;
    let end = (usize::BITS - minfo.hart_mask.leading_zeros()) as usize;
    log::trace!("{start}..{end}");

    for hartid in start..end {
        // Don't try to start ourselves
        if hartid == boot_hart {
            continue;
        }

        log::trace!("[{boot_hart}] starting hart {hartid}...");
        riscv::sbi::hsm::hart_start(
            hartid,
            _start_secondary as usize,
            minfo.fdt.as_ptr() as usize,
        )
        .map_err(Error::FailedToStartSecondaryHart)?;
    }

    Ok(())
}

pub unsafe fn map_contiguous(
    root_pgtable: usize,
    frame_alloc: &mut FrameAllocator,
    mut virt: usize,
    mut phys: usize,
    len: NonZero<usize>,
    flags: Flags,
    phys_off: usize,
) -> crate::Result<()> {
    let mut remaining_bytes = len.get();
    debug_assert!(
        remaining_bytes >= PAGE_SIZE,
        "address range span be at least one page"
    );
    debug_assert!(
        virt % PAGE_SIZE == 0,
        "virtual address must be aligned to at least 4KiB page size ({virt:#x})"
    );
    debug_assert!(
        phys % PAGE_SIZE == 0,
        "physical address must be aligned to at least 4KiB page size ({phys:#x})"
    );

    // To map out contiguous chunk of physical memory into the virtual address space efficiently
    // we'll attempt to map as much of the chunk using as large of a page size as possible.
    //
    // We'll follow the page table down starting at the root page table entry (PTE) and check at
    // every level if we can map there. This is dictated by the combination of virtual and
    // physical address alignment as well as chunk size. If we can map at the current level
    // well subtract the page size from `remaining_bytes`, advance the current virtual and physical
    // addresses by the page size and repeat the process until there are no more bytes to map.
    //
    // IF we can't map at a given level, we'll either allocate a new PTE or follow and existing PTE
    // to the next level (and therefore smaller page size) until we reach a level that we can map at.
    // Note that, because we require a minimum alignment and size of PAGE_SIZE, we will always be
    // able to map a chunk using level 0 pages.
    //
    // In effect this algorithm will map the start and ends of chunks using smaller page sizes
    // while mapping the vast majority of the middle of a chunk using larger page sizes.
    'outer: while remaining_bytes > 0 {
        let mut pgtable: NonNull<PageTableEntry> = pgtable_ptr_from_phys(root_pgtable, phys_off);

        for lvl in (0..PAGE_TABLE_LEVELS).rev() {
            let index = pte_index_for_level(virt, lvl);
            // Safety: index is always valid within a table
            let pte = unsafe { pgtable.add(index).as_mut() };

            if !pte.is_valid() {
                // If the PTE is invalid that means we reached a vacant slot to map into.
                //
                // First, lets check if we can map at this level of the page table given our
                // current virtual and physical address as well as the number of remaining bytes.
                if can_map_at_level(virt, phys, remaining_bytes, lvl) {
                    let page_size = page_size_for_level(lvl);

                    // This PTE is vacant AND we can map at this level
                    // mark this PTE as a valid leaf node pointing to the physical frame
                    pte.replace_address_and_flags(phys, PTEFlags::VALID | PTEFlags::from(flags));

                    virt = virt.checked_add(page_size).unwrap();
                    phys = phys.checked_add(page_size).unwrap();
                    remaining_bytes -= page_size;
                    continue 'outer;
                } else if !pte.is_valid() {
                    // The current PTE is vacant, but we couldn't map at this level (because the
                    // page size was too large, or the request wasn't sufficiently aligned or
                    // because the architecture just can't map at this level). This means
                    // we need to allocate a new sub-table and retry.
                    // allocate a new physical frame to hold the next level table and
                    // mark this PTE as a valid internal node pointing to that sub-table.
                    let frame = frame_alloc.allocate_one_zeroed(phys_off)?; // we should always be able to map a single page

                    // TODO memory barrier

                    pte.replace_address_and_flags(frame, PTEFlags::VALID);

                    pgtable = pgtable_ptr_from_phys(frame, phys_off);
                }
            } else if !pte.is_leaf() {
                // This PTE is an internal node pointing to another page table
                pgtable = pgtable_ptr_from_phys(pte.get_address_and_flags().0, phys_off);
            } else {
                unreachable!(
                    "Invalid state: PTE can't be valid leaf (this means {virt:#x} is already mapped) {pte:?} {pte:p}"
                );
            }
        }
    }

    Ok(())
}

pub unsafe fn remap_contiguous(
    root_pgtable: usize,
    mut virt: usize,
    mut phys: usize,
    len: NonZero<usize>,
    phys_off: usize,
) {
    let mut remaining_bytes = len.get();
    debug_assert!(
        remaining_bytes >= PAGE_SIZE,
        "virtual address range must span be at least one page"
    );
    debug_assert!(
        virt % PAGE_SIZE == 0,
        "virtual address must be aligned to at least 4KiB page size"
    );
    debug_assert!(
        phys % PAGE_SIZE == 0,
        "physical address must be aligned to at least 4KiB page size"
    );

    // This algorithm behaves a lot like the above one for `map_contiguous` but with an important
    // distinction: Since we require the entire range to already be mapped, we follow the page tables
    // until we reach a valid PTE. Once we reached, we assert that we can map the given physical
    // address here and simply update the PTEs address. We then again repeat this until we have
    // no more bytes to process.
    'outer: while remaining_bytes > 0 {
        // Safety: caller has to ensure root_pgtable is valid
        let mut pgtable = pgtable_ptr_from_phys(root_pgtable, phys_off);

        for lvl in (0..PAGE_TABLE_LEVELS).rev() {
            // Safety: index is always valid within a table
            let pte = unsafe {
                let index = pte_index_for_level(virt, lvl);
                pgtable.add(index).as_mut()
            };

            if pte.is_valid() && pte.is_leaf() {
                // We reached the previously mapped leaf node that we want to edit
                // assert that we can actually map at this level (remap requires users to remap
                // only to equal or larger alignments, but we should make sure.
                let page_size = page_size_for_level(lvl);

                debug_assert!(
                    can_map_at_level(virt, phys, remaining_bytes, lvl),
                    "remapping requires the same alignment ({page_size}) but found {phys:?}, {remaining_bytes}bytes"
                );

                let (_old_phys, flags) = pte.get_address_and_flags();
                pte.replace_address_and_flags(phys, flags);

                virt = virt.checked_add(page_size).unwrap();
                phys = phys.checked_add(page_size).unwrap();
                remaining_bytes -= page_size;
                continue 'outer;
            } else if pte.is_valid() {
                // This PTE is an internal node pointing to another page table
                pgtable = pgtable_ptr_from_phys(pte.get_address_and_flags().0, phys_off);
            } else {
                unreachable!("Invalid state: PTE cant be vacant or invalid+leaf {pte:?}");
            }
        }
    }
}

pub unsafe fn activate_aspace(pgtable: usize) {
    // Safety: register access
    unsafe {
        let ppn = pgtable >> 12_i32;
        satp::set(satp::Mode::Sv39, DEFAULT_ASID, ppn);
    }
}

/// Return the page size for the given page table level.
///
/// # Panics
///
/// Panics if the provided level is `>= PAGE_TABLE_LEVELS`.
pub fn page_size_for_level(level: usize) -> usize {
    assert!(level < PAGE_TABLE_LEVELS);
    let page_size = 1 << (PAGE_SHIFT + level * PAGE_ENTRY_SHIFT);
    debug_assert!(page_size == 4096 || page_size == 2097152 || page_size == 1073741824);
    page_size
}

/// Parse the `level`nth page table entry index from the given virtual address.
///
/// # Panics
///
/// Panics if the provided level is `>= PAGE_TABLE_LEVELS`.
pub fn pte_index_for_level(virt: usize, lvl: usize) -> usize {
    assert!(lvl < PAGE_TABLE_LEVELS);
    let index = (virt >> (PAGE_SHIFT + lvl * PAGE_ENTRY_SHIFT)) & (PAGE_TABLE_ENTRIES - 1);
    debug_assert!(index < PAGE_TABLE_ENTRIES);

    index
}

/// Return whether the combination of `virt`,`phys`, and `remaining_bytes` can be mapped at the given `level`.
///
/// This is the case when both the virtual and physical address are aligned to the page size at this level
/// AND the remaining size is at least the page size.
pub fn can_map_at_level(virt: usize, phys: usize, remaining_bytes: usize, lvl: usize) -> bool {
    let page_size = page_size_for_level(lvl);
    virt % page_size == 0 && phys % page_size == 0 && remaining_bytes >= page_size
}

fn pgtable_ptr_from_phys(phys: usize, phys_off: usize) -> NonNull<PageTableEntry> {
    NonNull::new(phys_off.checked_add(phys).unwrap() as *mut PageTableEntry).unwrap()
}

#[repr(transparent)]
pub struct PageTableEntry {
    bits: usize,
}

impl fmt::Debug for PageTableEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rsw = (self.bits & ((1 << 2_i32) - 1) << 8_i32) >> 8_i32;
        let ppn0 = (self.bits & ((1 << 9_i32) - 1) << 10_i32) >> 10_i32;
        let ppn1 = (self.bits & ((1 << 9_i32) - 1) << 19_i32) >> 19_i32;
        let ppn2 = (self.bits & ((1 << 26_i32) - 1) << 28_i32) >> 28_i32;
        let reserved = (self.bits & ((1 << 7_i32) - 1) << 54_i32) >> 54_i32;
        let pbmt = (self.bits & ((1 << 2_i32) - 1) << 61_i32) >> 61_i32;
        let n = (self.bits & ((1 << 1_i32) - 1) << 63_i32) >> 63_i32;

        f.debug_struct("PageTableEntry")
            .field("n", &format_args!("{n:01b}"))
            .field("pbmt", &format_args!("{pbmt:02b}"))
            .field("reserved", &format_args!("{reserved:07b}"))
            .field("ppn2", &format_args!("{ppn2:026b}"))
            .field("ppn1", &format_args!("{ppn1:09b}"))
            .field("ppn0", &format_args!("{ppn0:09b}"))
            .field("rsw", &format_args!("{rsw:02b}"))
            .field("flags", &self.get_address_and_flags().1)
            .finish()
    }
}

impl PageTableEntry {
    pub fn is_valid(&self) -> bool {
        PTEFlags::from_bits_retain(self.bits).contains(PTEFlags::VALID)
    }

    pub fn is_leaf(&self) -> bool {
        PTEFlags::from_bits_retain(self.bits)
            .intersects(PTEFlags::READ | PTEFlags::WRITE | PTEFlags::EXECUTE)
    }

    pub fn replace_address_and_flags(&mut self, address: usize, flags: PTEFlags) {
        self.bits &= PTEFlags::all().bits(); // clear all previous flags
        self.bits |= (address >> PTE_PPN_SHIFT) | flags.bits();
    }

    pub fn get_address_and_flags(&self) -> (usize, PTEFlags) {
        // TODO correctly mask out address
        let addr = (self.bits & !PTEFlags::all().bits()) << PTE_PPN_SHIFT;
        let flags = PTEFlags::from_bits_truncate(self.bits);
        (addr, flags)
    }
}

bitflags! {
    #[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
    pub struct PTEFlags: usize {
        const VALID     = 1 << 0;
        const READ      = 1 << 1;
        const WRITE     = 1 << 2;
        const EXECUTE   = 1 << 3;
        const USER      = 1 << 4;
        const GLOBAL    = 1 << 5;
        const ACCESSED    = 1 << 6;
        const DIRTY     = 1 << 7;
    }
}

impl From<Flags> for PTEFlags {
    fn from(flags: Flags) -> Self {
        let mut out = Self::VALID | Self::DIRTY | Self::ACCESSED;

        for flag in flags {
            match flag {
                Flags::READ => out.insert(Self::READ),
                Flags::WRITE => out.insert(Self::WRITE),
                Flags::EXECUTE => out.insert(Self::EXECUTE),
                _ => unreachable!(),
            }
        }

        out
    }
}

impl From<PTEFlags> for Flags {
    fn from(arch_flags: PTEFlags) -> Self {
        let mut out = Flags::empty();

        for flag in arch_flags {
            match flag {
                PTEFlags::READ => out.insert(Self::READ),
                PTEFlags::WRITE => out.insert(Self::WRITE),
                PTEFlags::EXECUTE => out.insert(Self::EXECUTE),
                _ => unreachable!(),
            }
        }

        out
    }
}
