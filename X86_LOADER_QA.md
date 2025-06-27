# x86_64 Loader Implementation Q&A

## Q: Why is the kernel image target triple "riscv64gc-unknown-none-elf" for RISC-V but "x86_64-unknown-none" for x86? Where is the missing "elf" suffix?
**A:** This is just a Rust naming convention difference. Both produce ELF binaries. The "elf" suffix in target triples is optional and doesn't affect the output format.

## Q: Which Rust source code produces the loader binaries?
**A:** Both architectures compile the same `/loader/src/` code with different targets:
- RISC-V: `target/riscv64gc-unknown-none-elf/debug/loader`
- x86_64: `target/x86_64-unknown-none/debug/loader`

## Q: Why is QEMU reporting "Error loading uncompressed kernel without PVH ELF Note"?
**A:** x86_64 QEMU expects specific boot protocols (Linux boot protocol, PVH, or UEFI) unlike RISC-V which directly loads ELF files. The PVH (Para-Virtualized Hardware) protocol requires a special ELF note.

## Q: How do Twizzler and Stramash handle x86_64 boot?
**A:** After analyzing both projects:
- Twizzler uses custom bootloader with multiboot2
- Stramash uses BOOTBOOT protocol
- Both avoid QEMU's direct kernel loading
I recommended PVH as the simplest solution most similar to RISC-V's direct loading.

## Q: What should be done in the _start function?
**A:** The _start function needs to:
1. Set up stack
2. Clear BSS (uninitialized globals)
3. Prepare arguments for main()
4. Call main()

## Q: What is BSS?
**A:** BSS (Block Started by Symbol) is the segment for uninitialized global variables that must be zeroed before main() runs.

## Q: What about the "consider rename libs/x86_64 to libs/x86" suggestion?
**A:** I renamed the crate to avoid naming conflicts between the x86_64 module and crate names.

## Q: What is "hio" in RISC-V?
**A:** HIO stands for "Host I/O" - it provides semihosting functionality for embedded/kernel code to communicate with the host system (like QEMU) for I/O operations, particularly useful for debugging output before proper drivers are available.

## Q: Should we move x86_64_print into libs/x86?
**A:** Yes, I created `libs/x86/src/serial.rs` to match the pattern used by RISC-V's `hio.rs`, providing serial port I/O for early boot debugging.

## Q: Is the PVH issue related to PIE?
**A:** Yes! Web search confirmed that Position Independent Executables (PIE) can cause issues with PVH boot protocol. PIE creates a DYN (shared object) instead of EXEC (executable) ELF type, which QEMU's PVH loader rejects.

## Q: Why create separate target specs for loader vs kernel?
**A:** The loader and kernel have fundamentally different requirements:
- **Loader**: Static linking at 0x100000, no PIE, simple panic=abort
- **Kernel**: PIE for KASLR, kernel code model for high memory, soft-float, unwinding

## Q: What was the reason for load_remaining_segments?
**A:** It was part of a complex two-stage loading process trying to work around PVH limitations by manually loading ELF segments that PVH didn't load. This was overly complex and had bugs, so I removed it.

## Q: Are we working on a two-stage loading process with PVH?
**A:** No, we're using single-stage direct boot. QEMU loads the entire loader ELF file at 0x100000, PVH jumps to _start, and _start does minimal setup before calling main().

## Q: Can you confirm we're not having missing segments?
**A:** ELF analysis confirms all segments are present:
- Type: EXEC (correct after disabling PIE)
- Single LOAD segment containing all sections (.text, .rodata, .data, .bss)
- Size: ~45MB loaded at 0x100000
- No missing segments!

## Q: Why are there both kernel and loader if we're doing single-stage boot?
**A:** k23 has two separate binaries (same as RISC-V):
1. **Loader**: Bootloader loaded by QEMU via PVH
2. **Kernel**: The actual microkernel (separate ELF embedded in loader)

"Single-stage" refers to the loader's boot process (PVH → _start → main), not the overall system architecture. The loader still needs to load and jump to the kernel.

## Current Status
- Successfully booting with "S1" output
- 'S' = _start reached
- '1' = CLI executed
- Crash occurs at segment register setup
- Next: Investigate x86_64 segment requirements