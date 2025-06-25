// Claude generated
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

// ============================================================================
// ARCHITECTURE CONSTANTS
// ============================================================================
// These mirror the RISC-V constants but adapted for x86_64:
// - x86_64 uses 4-level paging (PML4, PDPT, PD, PT) vs RISC-V's 3-level
// - Virtual addresses are 48-bit (sign extended to 64) vs RISC-V's 38-bit
// - Kernel space traditionally starts at 0xffff800000000000 on x86_64

pub const DEFAULT_ASID: u16 = 0; // x86_64 uses PCID instead of ASID, but same concept
pub const KERNEL_ASPACE_BASE: usize = 0xffff800000000000; // Canonical higher half
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_TABLE_ENTRIES: usize = 512; // Same as RISC-V
pub const PAGE_TABLE_LEVELS: usize = 4; // PML4, PDPT, PD, PT (vs RISC-V's 3)
pub const VIRT_ADDR_BITS: u32 = 48; // 48-bit virtual addresses (vs RISC-V's 38)

pub const PAGE_SHIFT: usize = (PAGE_SIZE - 1).count_ones() as usize;
pub const PAGE_ENTRY_SHIFT: usize = (PAGE_TABLE_ENTRIES - 1).count_ones() as usize;

// x86_64 doesn't shift physical address bits in PTEs like RISC-V does
// RISC-V shifts by 2, x86_64 uses full alignment
const PTE_ADDR_MASK: u64 = 0x000ffffffffff000; // Bits 12-51 for physical address

// ============================================================================
// INTERRUPT MODULE
// ============================================================================
// Provides interrupt control similar to RISC-V's CSR operations
pub mod interrupt {
    pub fn disable() {
        unsafe {
            core::arch::asm!("cli"); // Clear Interrupt Flag (vs RISC-V's csrc sstatus)
        }
    }

    pub fn enable() {
        unsafe {
            core::arch::asm!("sti"); // Set Interrupt Flag (vs RISC-V's csrs sstatus)
        }
    }
}

// ============================================================================
// BOOT ENTRY POINT
// ============================================================================
// This is the x86_64 equivalent of RISC-V's _start function
// Key differences:
// - x86_64 bootloaders (UEFI/multiboot) provide different boot info
// - No hart ID (CPU ID obtained differently on x86)
// - Different register conventions and stack setup

#[unsafe(link_section = ".text.start")]
#[unsafe(no_mangle)]
#[naked]
unsafe extern "C" fn _start() -> ! {
    // TODO: This is a minimal stub. Full implementation needs:
    // - Multiboot/UEFI header parsing
    // - GDT/IDT setup
    // - Stack setup per CPU
    // - BSS clearing
    // - Jump to Rust main
    unsafe {
        naked_asm! {
            // Disable interrupts (equivalent to RISC-V's csrc sstatus)
            "cli",

            // TODO: Get CPU ID (equivalent to RISC-V's hart ID in a0)
            // On x86_64, this requires CPUID instruction or APIC ID
            "xor rdi, rdi", // For now, assume CPU 0

            // TODO: Set up stack (equivalent to RISC-V's stack calculation)
            // "mov rsp, offset __stack_start",
            // "add rsp, STACK_SIZE",

            // TODO: Clear BSS (similar to RISC-V's BSS clearing loop)

            // TODO: Call Rust entry point
            // "call main",

            // Halt if we somehow return
            "2:",
            "hlt",
            "jmp 2b",
        }
    }
}

// ============================================================================
// SECONDARY CPU ENTRY
// ============================================================================
// x86_64 equivalent of _start_secondary for Application Processors (APs)
// On x86_64, APs are started via INIT-SIPI-SIPI sequence, not SBI like RISC-V
#[naked]
unsafe extern "C" fn _start_secondary() -> ! {
    // TODO: Implement AP startup
    // This is more complex on x86_64 as it requires:
    // - Real mode to protected mode to long mode transition
    // - Per-CPU GDT/IDT setup
    // - Local APIC initialization
    unsafe {
        naked_asm! {
            "cli",
            "hlt",
        }
    }
}

// ============================================================================
// STACK CANARY FILL
// ============================================================================
// Direct port of RISC-V's fill_stack function
// Uses different registers but same logic
#[naked]
unsafe extern "C" fn fill_stack() {
    unsafe {
        naked_asm! {
            // Fill with 0xACE0BACE pattern
            // rdi = bottom of stack (like RISC-V's t0)
            // rsp = top of stack
            // "mov rax, 0xACE0BACE",
            // "1:",
            // "mov [rdi], rax",
            // "add rdi, 8",
            // "cmp rdi, rsp",
            // "jb 1b",
            "ret"
        }
    }
}

// ============================================================================
// KERNEL HANDOFF
// ============================================================================
// x86_64 equivalent of handoff_to_kernel
// Key differences:
// - Different calling convention (System V ABI vs RISC-V ABI)
// - No SUM bit equivalent (user memory access handled differently)
// - Different register usage for arguments
pub unsafe fn handoff_to_kernel(cpuid: usize, boot_ticks: u64, init: &GlobalInitResult) -> ! {
    // let stack = init.stacks_alloc.region_for_cpu(cpuid);
    // let tls = init
    //     .maybe_tls_alloc
    //     .as_ref()
    //     .map(|tls| tls.region_for_hart(cpuid))
    //     .unwrap_or_default();

    // log::debug!("CPU {cpuid} Jumping to kernel...");
    // log::trace!(
    //     "CPU {cpuid} entry: {:#x}, arguments: rdi={cpuid} rsi={:?} stack={stack:#x?} tls={tls:#x?}",
    //     init.kernel_entry,
    //     init.boot_info
    // );

    // init.barrier.wait();

    // unsafe {
    //     // x86_64 doesn't have an equivalent to RISC-V's sstatus.SUM bit
    //     // User memory access is controlled via page table permissions

    //     asm! {
    //         // Set up stack (RSP instead of RISC-V's SP)
    //         "mov rsp, {stack_top}",

    //         // TODO: Set up TLS (FS segment base instead of RISC-V's TP)
    //         // This requires MSR writes which need to be implemented

    //         // Fill stack with canary
    //         "mov rdi, {stack_bottom}",
    //         "call {fill_stack}",

    //         // Clear return address (same concept as RISC-V)
    //         "xor rax, rax",
    //         "push rax", // Push 0 as return address

    //         // Jump to kernel
    //         // x86_64 System V ABI: rdi, rsi, rdx for first 3 args
    //         // (vs RISC-V's a0, a1, a2)
    //         "jmp {kernel_entry}",

    //         // Should never reach here
    //         "1:",
    //         "hlt",
    //         "jmp 1b",

    //         in("rdi") cpuid,
    //         in("rsi") init.boot_info,
    //         in("rdx") boot_ticks,
    //         stack_bottom = in(reg) stack.start,
    //         stack_top = in(reg) stack.end,
    //         kernel_entry = in(reg) init.kernelexo_entry,
    //         fill_stack = sym fill_stack,
    //         options(noreturn)
    //     }
    // }
    todo!("handoff_to_kernel not implemented for x86_64");
    panic!("handoff_to_kernel not implemented for x86_64");
}

// ============================================================================
// SECONDARY CPU STARTUP
// ============================================================================
// x86_64 equivalent of start_secondary_harts
// Major differences:
// - Uses APIC and INIT-SIPI-SIPI instead of SBI HSM
// - More complex due to x86 legacy (real mode startup)
pub fn start_secondary_harts(boot_cpu: usize, minfo: &MachineInfo) -> crate::Result<()> {
    // TODO: Implement x86_64 MP startup
    // This requires:
    // 1. Setting up AP trampoline code in low memory
    // 2. Using Local APIC to send INIT-SIPI-SIPI sequence
    // 3. Synchronization via shared memory flags

    // For now, just return Ok as single CPU
    log::warn!("x86_64 SMP not yet implemented, running on single CPU");
    Ok(())
}

// ============================================================================
// PAGE TABLE MANAGEMENT
// ============================================================================
// This section implements x86_64 page table operations
// Key differences from RISC-V:
// - 4-level instead of 3-level
// - Different PTE format and flags
// - No address bit shifting in PTEs

/// Map a contiguous range of physical memory into virtual address space
/// This is a direct port of RISC-V's map_contiguous with x86_64 adaptations
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

    // Same algorithm as RISC-V but with 4 levels instead of 3
    'outer: while remaining_bytes > 0 {
        let mut pgtable: NonNull<PageTableEntry> = pgtable_ptr_from_phys(root_pgtable, phys_off);

        for lvl in (0..PAGE_TABLE_LEVELS).rev() {
            let index = pte_index_for_level(virt, lvl);
            let pte = unsafe { pgtable.add(index).as_mut() };

            if !pte.is_present() {
                if can_map_at_level(virt, phys, remaining_bytes, lvl) {
                    let page_size = page_size_for_level(lvl);

                    // Create leaf PTE
                    pte.set_leaf(phys, PageTableFlags::from(flags), lvl);

                    virt = virt.checked_add(page_size).unwrap();
                    phys = phys.checked_add(page_size).unwrap();
                    remaining_bytes -= page_size;
                    continue 'outer;
                } else {
                    // Allocate new page table
                    let frame = frame_alloc.allocate_one_zeroed(phys_off)?;

                    // TODO: Memory barrier (MFENCE on x86_64)

                    pte.set_table(frame);
                    pgtable = pgtable_ptr_from_phys(frame, phys_off);
                }
            } else if !pte.is_leaf() {
                // Follow to next level
                pgtable = pgtable_ptr_from_phys(pte.address(), phys_off);
            } else {
                unreachable!(
                    "Invalid state: PTE can't be valid leaf (this means {virt:#x} is already mapped)"
                );
            }
        }
    }

    Ok(())
}

/// Remap an already-mapped range to new physical addresses
/// Direct port of RISC-V's remap_contiguous
pub unsafe fn remap_contiguous(
    root_pgtable: usize,
    mut virt: usize,
    mut phys: usize,
    len: NonZero<usize>,
    phys_off: usize,
) {
    // Implementation follows same logic as RISC-V version
    // but uses x86_64 PTE format
    let mut remaining_bytes = len.get();

    'outer: while remaining_bytes > 0 {
        let mut pgtable = pgtable_ptr_from_phys(root_pgtable, phys_off);

        for lvl in (0..PAGE_TABLE_LEVELS).rev() {
            let index = pte_index_for_level(virt, lvl);
            let pte = unsafe { pgtable.add(index).as_mut() };

            if pte.is_present() && pte.is_leaf() {
                let page_size = page_size_for_level(lvl);

                debug_assert!(
                    can_map_at_level(virt, phys, remaining_bytes, lvl),
                    "remapping requires the same alignment"
                );

                let flags = pte.flags();
                pte.set_address(phys, flags);

                virt = virt.checked_add(page_size).unwrap();
                phys = phys.checked_add(page_size).unwrap();
                remaining_bytes -= page_size;
                continue 'outer;
            } else if pte.is_present() {
                pgtable = pgtable_ptr_from_phys(pte.address(), phys_off);
            } else {
                unreachable!("Invalid state: PTE cant be absent");
            }
        }
    }
}

/// Activate an address space by loading CR3
/// Equivalent to RISC-V's satp register write
pub unsafe fn activate_aspace(pgtable: usize) {
    unsafe {
        // x86_64: Load CR3 with page table base
        // Equivalent to RISC-V: satp::set(Mode::Sv39, ASID, ppn)
        asm!("mov cr3, {}", in(reg) pgtable);
    }
}

/// Get page size for a given level
/// x86_64 levels: 0=1GB (PDPT), 1=2MB (PD), 2=4KB (PT), 3=4KB (PML4 doesn't have pages)
pub fn page_size_for_level(level: usize) -> usize {
    assert!(level < PAGE_TABLE_LEVELS);
    match level {
        0 => 1 << 30, // 1GB pages
        1 => 1 << 21, // 2MB pages
        2 => 1 << 12, // 4KB pages
        3 => 1 << 12, // PML4 level (no direct pages)
        _ => unreachable!(),
    }
}

/// Extract page table index from virtual address for given level
/// Same concept as RISC-V but adapted for 4-level paging
pub fn pte_index_for_level(virt: usize, lvl: usize) -> usize {
    assert!(lvl < PAGE_TABLE_LEVELS);
    // Each level uses 9 bits of the virtual address
    let index = (virt >> (PAGE_SHIFT + lvl * PAGE_ENTRY_SHIFT)) & (PAGE_TABLE_ENTRIES - 1);
    debug_assert!(index < PAGE_TABLE_ENTRIES);
    index
}

/// Check if we can map at this level given alignment and size
/// Same logic as RISC-V
pub fn can_map_at_level(virt: usize, phys: usize, remaining_bytes: usize, lvl: usize) -> bool {
    let page_size = page_size_for_level(lvl);
    virt % page_size == 0 && phys % page_size == 0 && remaining_bytes >= page_size
}

fn pgtable_ptr_from_phys(phys: usize, phys_off: usize) -> NonNull<PageTableEntry> {
    NonNull::new(phys_off.checked_add(phys).unwrap() as *mut PageTableEntry).unwrap()
}

// ============================================================================
// PAGE TABLE ENTRY
// ============================================================================
// x86_64 PTE format is different from RISC-V:
// - Bits 0-11: Flags
// - Bits 12-51: Physical address (no shifting like RISC-V)
// - Bits 52-62: Available/ignored
// - Bit 63: NX (No Execute)

#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct PageTableEntry {
    bits: u64,
}

impl PageTableEntry {
    pub fn is_present(&self) -> bool {
        self.bits & PageTableFlags::PRESENT.bits() != 0
    }

    pub fn is_leaf(&self) -> bool {
        // In x86_64, leaf is determined by:
        // - HUGE bit for large pages (2MB, 1GB)
        // - Or being at the lowest level (PT)
        // This is different from RISC-V which uses R/W/X bits
        self.is_present() && (self.bits & PageTableFlags::HUGE.bits() != 0)
    }

    pub fn address(&self) -> usize {
        (self.bits & PTE_ADDR_MASK) as usize
    }

    pub fn flags(&self) -> PageTableFlags {
        PageTableFlags::from_bits_truncate(self.bits)
    }

    pub fn set_leaf(&mut self, phys: usize, flags: PageTableFlags, level: usize) {
        self.bits = (phys as u64 & PTE_ADDR_MASK) | flags.bits();
        // Set HUGE bit for large pages (levels 0 and 1)
        if level < 2 {
            self.bits |= PageTableFlags::HUGE.bits();
        }
    }

    pub fn set_table(&mut self, table_addr: usize) {
        // For table entries, set minimal permissions
        self.bits = (table_addr as u64 & PTE_ADDR_MASK)
            | PageTableFlags::PRESENT.bits()
            | PageTableFlags::WRITABLE.bits()
            | PageTableFlags::USER.bits();
    }

    pub fn set_address(&mut self, phys: usize, flags: PageTableFlags) {
        self.bits = (phys as u64 & PTE_ADDR_MASK) | flags.bits();
    }
}

impl fmt::Debug for PageTableEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.is_present() {
            write!(f, "PageTableEntry::NotPresent")
        } else if self.is_leaf() {
            write!(
                f,
                "PageTableEntry::Leaf(0x{:x}, {:?})",
                self.address(),
                self.flags()
            )
        } else {
            write!(f, "PageTableEntry::Table(0x{:x})", self.address())
        }
    }
}

// ============================================================================
// PAGE TABLE FLAGS
// ============================================================================
// x86_64 page table flags - very different from RISC-V
bitflags! {
    #[derive(Debug, Copy, Clone, Eq, PartialEq)]
    pub struct PageTableFlags: u64 {
        const PRESENT = 1 << 0;       // Valid bit (like RISC-V VALID)
        const WRITABLE = 1 << 1;      // Write permission (like RISC-V WRITE)
        const USER = 1 << 2;          // User accessible (like RISC-V USER)
        const WRITE_THROUGH = 1 << 3; // Cache policy
        const NO_CACHE = 1 << 4;      // Cache disable
        const ACCESSED = 1 << 5;      // Like RISC-V ACCESSED
        const DIRTY = 1 << 6;         // Like RISC-V DIRTY
        const HUGE = 1 << 7;          // Large page (2MB/1GB)
        const GLOBAL = 1 << 8;        // Like RISC-V GLOBAL
        const NO_EXECUTE = 1 << 63;   // NX bit (inverse of RISC-V EXECUTE)
    }
}

/// Convert generic flags to x86_64 specific flags
/// Key difference: x86_64 uses NO_EXECUTE bit vs RISC-V's EXECUTE bit
impl From<Flags> for PageTableFlags {
    fn from(flags: Flags) -> Self {
        // let mut result = PageTableFlags::PRESENT | PageTableFlags::ACCESSED | PageTableFlags::DIRTY;

        // if flags.contains(Flags::WRITE) {
        //     result |= PageTableFlags::WRITABLE;
        // }

        // if flags.contains(Flags::USER) {
        //     result |= PageTableFlags::USER;
        // }

        // // Note the inversion: RISC-V has EXECUTE, x86_64 has NO_EXECUTE
        // if !flags.contains(Flags::EXECUTE) {
        //     result |= PageTableFlags::NO_EXECUTE;
        // }

        // // TODO: Handle DEVICE flag for MMIO (set NO_CACHE)

        // result
        
        todo!("From<Flags> for PageTableFlags not implemented for x86_64");
        panic!("From<Flags> for PageTableFlags not implemented for x86_64");
    }
}

impl From<PageTableFlags> for Flags {
    fn from(arch_flags: PageTableFlags) -> Self {
        // let mut out = Flags::empty();

        // // Always readable on x86_64 if present
        // if arch_flags.contains(PageTableFlags::PRESENT) {
        //     out.insert(Flags::READ);
        // }

        // if arch_flags.contains(PageTableFlags::WRITABLE) {
        //     out.insert(Flags::WRITE);
        // }

        // // Note the inversion again
        // if !arch_flags.contains(PageTableFlags::NO_EXECUTE) {
        //     out.insert(Flags::EXECUTE);
        // }

        // if arch_flags.contains(PageTableFlags::USER) {
        //     out.insert(Flags::USER);
        // }

        // out

        todo!("From<PageTableFlags> for Flags not implemented for x86_64");
        panic!("From<PageTableFlags> for Flags not implemented for x86_64");
    }
}
