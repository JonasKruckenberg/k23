/*
 * riscv64 has no upstream `*-unknown-uefi` rustc target, so we compile for
 * `riscv64gc-unknown-none-elf` and drive lld in GNU flavor via this script to
 * produce a position-independent ELF shared object. `//build/riscv-elf2efi`
 * then converts it to PE.
 *
 * elf2efi only cares about PROGBITS/NOBITS sections with SHF_ALLOC. The
 * dynamic-linking metadata that -shared generates (.dynsym, .gnu.hash, etc.)
 * is discarded during PE conversion — but must still be laid out validly in
 * the ELF. We place runtime sections first (page-aligned for PE) and the
 * metadata sections after, so their large sizes don't create file-offset
 * conflicts with orphan non-alloc sections.
 */

OUTPUT_FORMAT("elf64-littleriscv")
OUTPUT_ARCH(riscv)

SECTIONS {
    /* --- runtime sections (become PE .text / .data) --- */

    /* Leave room for PE headers — elf2efi writes DOS+NT+section headers into
       the virtual space before the first section. */
    . = SIZEOF_HEADERS;
    . = ALIGN(4096);
    .text : {
        __text_start = .;
        *(.text .text.*)
        . = ALIGN(4096);
        __text_end = .;
    }

    . = ALIGN(4096);
    .rodata : {
        __rodata_start = .;
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        *(.eh_frame .eh_frame.*)
        . = ALIGN(4096);
        __rodata_end = .;
    }

    . = ALIGN(4096);
    .data : {
        __data_start = .;
        *(.got .got.plt .igot .igot.plt)
        *(.data.rel.ro .data.rel.ro.*)
        *(.data .data.*)
        *(.sdata .sdata.*)
        . = ALIGN(4096);
        __data_end = .;
    }

    . = ALIGN(4096);
    .bss (NOLOAD) : {
        __bss_start = .;
        *(.bss.uninit)
        . = ALIGN(8);
        __bss_zero_start = .;
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    }

    __stack_start = .;

    /* --- dynamic-linking metadata (discarded by elf2efi) --- */

    .dynamic    : { *(.dynamic) }
    .dynsym     : { *(.dynsym) }
    .gnu.hash   : { *(.gnu.hash) }
    .hash       : { *(.hash) }
    .dynstr     : { *(.dynstr) }
    .rela.dyn   : { *(.rela.dyn .rela.*) }

    /DISCARD/ : {
        *(.note.GNU-stack)
        *(.comment)
        *(.gnu.lto_*)
        *(.riscv.attributes)
    }
}
