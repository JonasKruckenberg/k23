// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::GlobalInitResult;
use crate::frame_alloc::FrameAllocator;
use crate::machine_info::MachineInfo;
use crate::mapping::Flags;
use bitflags::bitflags;
use core::arch::{asm, global_asm, naked_asm};
use core::fmt;
use core::num::NonZero;
use core::ptr::NonNull;

// PVH ELF Note to enable direct kernel loading like RISC-V
// This allows QEMU to boot our kernel directly without a traditional bootloader
global_asm!(
    r#"
    .pushsection .note.Xen, "a", @note
    .align 4
    .long 4                    /* name size */
    .long 4                    /* desc size */
    .long 0x12                 /* type = XEN_ELFNOTE_PHYS32_ENTRY */
    .asciz "Xen"              /* name */
    .long 0x100000             /* desc = entry point at 1MB physical */
    .popsection
    "#
);

pub const DEFAULT_ASID: u16 = 0;
pub const KERNEL_ASPACE_BASE: usize = 0xffffffc000000000;
pub const PAGE_SIZE: usize = 4096;
pub const PAGE_TABLE_ENTRIES: usize = 512;
pub const PAGE_TABLE_LEVELS: usize = 4; // PML4, PDPT, PD, PT
pub const VIRT_ADDR_BITS: u32 = 48;

pub const PAGE_SHIFT: usize = (PAGE_SIZE - 1).count_ones() as usize;
pub const PAGE_ENTRY_SHIFT: usize = (PAGE_TABLE_ENTRIES - 1).count_ones() as usize;

/// Entry point for the boot CPU
/// PVH boot protocol starts us in 32-bit protected mode with:
/// - ebx = boot params address (we ignore for simplicity)
/// - All other registers undefined
/// We need to transition to 64-bit long mode before running 64-bit code
#[unsafe(link_section = ".text.start")]
#[unsafe(no_mangle)]
#[naked]
unsafe extern "C" fn _start() -> ! {
    unsafe {
        naked_asm! {
            // We start in 32-bit protected mode, need to transition to 64-bit
            ".code32",

            // Disable interrupts
            "cli",

            // Enable PAE (required for long mode)
            "mov eax, cr4",
            "or eax, 0x20",      // Set PAE bit
            "mov cr4, eax",

            // Set up initial page tables for identity mapping
            // We'll create a minimal identity map for the first 4GB
            // PML4 at 0x1000, PDPT at 0x2000, PD at 0x3000

            // Clear page table area
            "mov edi, 0x1000",
            "xor eax, eax",
            "mov ecx, 0x3000",   // Clear 12KB (3 pages)
            "rep stosd",

            // Set up PML4[0] -> PDPT
            "mov dword ptr [0x1000], 0x2003",  // PDPT address | Present | Writable

            // Set up PDPT[0] -> PD
            "mov dword ptr [0x2000], 0x3003",  // PD address | Present | Writable

            // Set up PD entries for first 1GB (512 * 2MB pages)
            // Using 2MB pages (bit 7 = PS)
            "mov edi, 0x3000",
            "mov eax, 0x83",     // Present | Writable | PS (2MB pages)
            "mov ecx, 512",      // 512 entries
            "2:",
            "mov [edi], eax",
            "add eax, 0x200000", // Next 2MB
            "add edi, 8",
            "loop 2b",

            // Load PML4 into CR3
            "mov eax, 0x1000",
            "mov cr3, eax",

            // Enable long mode in EFER MSR
            // WRMSR uses: ECX = MSR number, EDX:EAX = value
            "mov ecx, 0xC0000080",  // EFER MSR number
            "mov eax, 0x900",       // LME + NXE
            "xor edx, edx",         // Upper 32 bits = 0
            "wrmsr",

            // Enable paging and protected mode
            "mov eax, cr0",
            "or eax, 0x80000001",   // Set PG and PE bits
            "mov cr0, eax",

            // Load a temporary GDT with 64-bit code segment
            "lgdt [5f]",         // Load GDT descriptor at label 5

            // Far jump to 64-bit code
            // Use manual encoding for far jump
            ".byte 0xEA",        // Far jump opcode
            ".long 6f",          // Offset to label 6
            ".word 0x08",        // Code segment selector

            // GDT descriptor (label 5)
            ".align 4",
            "5:",
            ".word 23",          // GDT limit (3 entries * 8 - 1)
            ".long 4f",          // GDT base address (label 4)

            // Temporary GDT (label 4)
            ".align 16",
            "4:",
            ".quad 0",                          // Null descriptor
            ".quad 0x00af9a000000ffff",        // 64-bit code segment
            ".quad 0x00cf92000000ffff",        // Data segment

            // Now in 64-bit mode (label 6)
            ".code64",
            "6:",

            // Set up data segments for 64-bit mode
            "mov ax, 0x10",      // Data segment selector
            "mov ds, ax",
            "mov es, ax",
            "mov fs, ax",
            "mov gs, ax",
            "mov ss, ax",

            // Set up a temporary stack at a known good location
            // Use 2MB as temporary stack location (well above our code at 1MB)
            "mov rsp, 0x200000",

            // Clear direction flag for string operations
            "cld",

            // Clear BSS section (uninitialized globals)
            "lea rdi, [rip + __bss_zero_start]",
            "lea rcx, [rip + __bss_end]",
            "sub rcx, rdi",      // rcx = size of BSS
            "jz 3f",             // Skip if BSS is empty
            "xor eax, eax",      // Zero to write
            "rep stosb",         // Clear BSS byte by byte
            "3:",

            // Output '!' before calling main
            "mov al, 0x21",      // '!'
            "mov dx, 0x3F8",     // COM1 port
            "out dx, al",

            // Set up arguments for main()
            "xor rdi, rdi",      // CPU ID = 0
            "xor rsi, rsi",      // No FDT on x86
            "xor rdx, rdx",      // boot_ticks = 0

            // Call Rust main
            "call {main}",

            // Should never return
            "2:",
            "hlt",
            "jmp 2b",

            main = sym crate::main,
        }
    }
}

/// Entry point for secondary CPUs (not implemented yet)
#[naked]
unsafe extern "C" fn _start_secondary() -> ! {
    unsafe {
        naked_asm! {
            // For now, just halt secondary CPUs
            "cli",
            "2:",
            "hlt",
            "jmp 2b"
        }
    }
}

/// Fill stack with canary pattern
/// rdi = bottom of stack, rsi = top of stack
#[naked]
unsafe extern "C" fn fill_stack() {
    unsafe {
        naked_asm! {
            "mov rax, 0xACE0BACE",
            "2:",
            "mov [rdi], rax",
            "add rdi, 8",
            "cmp rdi, rsi",
            "jb 2b",
            "ret"
        }
    }
}

/// Hand off control to the kernel
pub unsafe fn handoff_to_kernel(cpuid: usize, boot_ticks: u64, init: &GlobalInitResult) -> ! {
    let stack = init.stacks_alloc.region_for_cpu(cpuid);
    let tls = init
        .maybe_tls_alloc
        .as_ref()
        .map(|tls| tls.region_for_hart(cpuid))
        .unwrap_or_default();

    log::debug!("CPU {cpuid} Jumping to kernel...");
    log::trace!(
        "CPU {cpuid} entry: {:#x}, arguments: rdi={cpuid} rsi={:?} stack={stack:#x?} tls={tls:#x?}",
        init.kernel_entry,
        init.boot_info
    );

    init.barrier.wait();

    unsafe {
        asm! {
            // Set up stack
            "mov rsp, {stack_top}",

            // Set up TLS (FS base)
            "mov rcx, 0xc0000100",  // FS_BASE MSR
            "mov rax, {tls_start}",
            "mov rdx, {tls_start}",
            "shr rdx, 32",
            "wrmsr",

            // Fill stack with canary
            "mov rdi, {stack_bottom}",
            "mov rsi, {stack_top}",
            "call {fill_stack}",

            // Clear return address
            "xor rax, rax",

            // Jump to kernel (System V ABI)
            "jmp {kernel_entry}",

            in("rdi") cpuid,
            in("rsi") init.boot_info,
            in("rdx") boot_ticks,
            stack_bottom = in(reg) stack.start,
            stack_top = in(reg) stack.end,
            tls_start = in(reg) tls.start,
            kernel_entry = in(reg) init.kernel_entry,
            fill_stack = sym fill_stack,
            options(noreturn)
        }
    }
}

/// Start secondary CPUs (not implemented yet)
pub fn start_secondary_harts(boot_cpu: usize, _minfo: &MachineInfo) -> crate::Result<()> {
    log::warn!("x86_64 SMP not yet implemented, running on single CPU");
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

    // Round up to page size if less than a page
    if remaining_bytes < PAGE_SIZE {
        remaining_bytes = PAGE_SIZE;
    }

    debug_assert!(
        virt % PAGE_SIZE == 0,
        "virtual address must be page-aligned: {:#x}",
        virt
    );
    debug_assert!(
        phys % PAGE_SIZE == 0,
        "physical address must be page-aligned: {:#x}",
        phys
    );

    'outer: while remaining_bytes > 0 {
        let mut pgtable: NonNull<PageTableEntry> = pgtable_ptr_from_phys(root_pgtable, phys_off);

        for lvl in (0..PAGE_TABLE_LEVELS).rev() {
            let index = pte_index_for_level(virt, lvl);
            let pte = unsafe { pgtable.add(index).as_mut() };

            if !pte.is_valid() {
                // For partial pages at the end, we still need to map a full page
                let effective_remaining = if remaining_bytes < PAGE_SIZE && lvl == 0 {
                    PAGE_SIZE
                } else {
                    remaining_bytes
                };

                if can_map_at_level(virt, phys, effective_remaining, lvl) {
                    let page_size = page_size_for_level(lvl);
                    pte.replace_address_and_flags(phys, PTEFlags::VALID | PTEFlags::from(flags));

                    virt = virt.checked_add(page_size).unwrap();
                    phys = phys.checked_add(page_size).unwrap();
                    remaining_bytes = remaining_bytes.saturating_sub(page_size);
                    continue 'outer;
                } else {
                    let frame = frame_alloc.allocate_one_zeroed(phys_off)?;
                    pte.replace_address_and_flags(frame, PTEFlags::VALID);
                    pgtable = pgtable_ptr_from_phys(frame, phys_off);
                }
            } else if !pte.is_leaf() {
                pgtable = pgtable_ptr_from_phys(pte.get_address_and_flags().0, phys_off);
            } else {
                unreachable!("Invalid state: {virt:#x} is already mapped");
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
    debug_assert!(remaining_bytes >= PAGE_SIZE);
    debug_assert!(virt % PAGE_SIZE == 0);
    debug_assert!(phys % PAGE_SIZE == 0);

    'outer: while remaining_bytes > 0 {
        let mut pgtable = pgtable_ptr_from_phys(root_pgtable, phys_off);

        for lvl in (0..PAGE_TABLE_LEVELS).rev() {
            let index = pte_index_for_level(virt, lvl);
            let pte = unsafe { pgtable.add(index).as_mut() };

            if pte.is_valid() && pte.is_leaf() {
                let page_size = page_size_for_level(lvl);
                debug_assert!(can_map_at_level(virt, phys, remaining_bytes, lvl));

                let (_old_phys, flags) = pte.get_address_and_flags();
                pte.replace_address_and_flags(phys, flags);

                virt = virt.checked_add(page_size).unwrap();
                phys = phys.checked_add(page_size).unwrap();
                remaining_bytes -= page_size;
                continue 'outer;
            } else if pte.is_valid() {
                pgtable = pgtable_ptr_from_phys(pte.get_address_and_flags().0, phys_off);
            } else {
                unreachable!("Invalid state");
            }
        }
    }
}

pub unsafe fn activate_aspace(pgtable: usize) {
    unsafe {
        asm!("mov cr3, {}", in(reg) pgtable);
    }
}

pub fn page_size_for_level(level: usize) -> usize {
    assert!(level < PAGE_TABLE_LEVELS);
    // x86_64 uses 4-level paging, but we map it to match RISC-V's expectations
    // Level 3 (PML4) and 2 (PDPT) can't have leaf pages in our usage
    match level {
        0 => 1 << 12, // 4KB pages (PT level)
        1 => 1 << 21, // 2MB pages (PD level)
        2 => 1 << 30, // 1GB pages (PDPT level)
        3 => 1 << 30, // PML4 doesn't have pages, return 1GB
        _ => unreachable!(),
    }
}

pub fn pte_index_for_level(virt: usize, lvl: usize) -> usize {
    assert!(lvl < PAGE_TABLE_LEVELS);
    let index = (virt >> (PAGE_SHIFT + lvl * PAGE_ENTRY_SHIFT)) & (PAGE_TABLE_ENTRIES - 1);
    debug_assert!(index < PAGE_TABLE_ENTRIES);
    index
}

pub fn can_map_at_level(virt: usize, phys: usize, remaining_bytes: usize, lvl: usize) -> bool {
    // Don't allow leaf pages at PML4 level
    if lvl >= 3 {
        return false;
    }
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

impl PageTableEntry {
    pub fn is_valid(&self) -> bool {
        PTEFlags::from_bits_retain(self.bits).contains(PTEFlags::VALID)
    }

    pub fn is_leaf(&self) -> bool {
        // For x86_64, a page is a leaf if it has R/W/X-like permissions
        // or the PS (page size) bit for large pages
        PTEFlags::from_bits_retain(self.bits)
            .intersects(PTEFlags::WRITABLE | PTEFlags::USER | PTEFlags::HUGE)
    }

    pub fn replace_address_and_flags(&mut self, address: usize, flags: PTEFlags) {
        self.bits = 0;
        self.bits |= (address & !0xFFF) | flags.bits();
    }

    pub fn get_address_and_flags(&self) -> (usize, PTEFlags) {
        let addr = self.bits & !0xFFF;
        let flags = PTEFlags::from_bits_truncate(self.bits);
        (addr, flags)
    }
}

impl fmt::Debug for PageTableEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (addr, flags) = self.get_address_and_flags();
        f.debug_struct("PageTableEntry")
            .field("addr", &format_args!("{addr:#x}"))
            .field("flags", &flags)
            .finish()
    }
}

bitflags! {
    #[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
    pub struct PTEFlags: usize {
        const VALID = 1 << 0;      // Present bit
        const WRITABLE = 1 << 1;   // Write permission
        const USER = 1 << 2;       // User accessible
        const PWT = 1 << 3;        // Write through
        const PCD = 1 << 4;        // Cache disable
        const ACCESSED = 1 << 5;   // Accessed bit
        const DIRTY = 1 << 6;      // Dirty bit
        const HUGE = 1 << 7;       // Page size bit
        const GLOBAL = 1 << 8;     // Global page
        const NX = 1 << 63;        // No execute
    }
}

impl From<Flags> for PTEFlags {
    fn from(flags: Flags) -> Self {
        let mut out = Self::VALID | Self::DIRTY | Self::ACCESSED;

        // x86_64 is always readable if present
        if flags.contains(Flags::WRITE) {
            out.insert(Self::WRITABLE);
        }

        // Note: x86_64 uses NX bit (inverse of execute)
        if !flags.contains(Flags::EXECUTE) {
            out.insert(Self::NX);
        }

        out
    }
}

impl From<PTEFlags> for Flags {
    fn from(arch_flags: PTEFlags) -> Self {
        let mut out = Flags::empty();

        // x86_64 pages are always readable if valid
        if arch_flags.contains(PTEFlags::VALID) {
            out.insert(Self::READ);
        }

        if arch_flags.contains(PTEFlags::WRITABLE) {
            out.insert(Self::WRITE);
        }

        // Note the inversion
        if !arch_flags.contains(PTEFlags::NX) {
            out.insert(Self::EXECUTE);
        }

        out
    }
}
