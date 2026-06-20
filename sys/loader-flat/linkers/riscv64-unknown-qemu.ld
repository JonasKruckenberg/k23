OUTPUT_ARCH(riscv)
ENTRY(_start)

LOADER_OFFSET = 0x80200000;

SECTIONS {
    . = LOADER_OFFSET;
    . = ALIGN(4096);
    __loader_start = .;

    .text : {
        *(.text.start)

        . = ALIGN(4096);
        __handoff_trampoline_start = .;
        KEEP(*(.text.handoff_trampoline))
        . = ALIGN(4096);
        __handoff_trampoline_end = .;

        *(.text .text.*)
    }

    .rodata : {
        *(.got .got.*)
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }

    .bss (NOLOAD) : {
        . = ALIGN(8); # 8-byte alignment for u64 ptr trick
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        __bss_end = .;
    }

    /* Uninitialised statics that must NOT be zeroed by the BSS-clear loop.
       Placed outside __bss_start/__bss_end on purpose. */
    .bss.uninit (NOLOAD) : {
        . = ALIGN(16);
        *(.bss.uninit)
    }

    .data : {
        PROVIDE(__global_pointer$ = . + 0x800);
        *(.data .data.*)
        *(.sdata .sdata.*)
    }

    . = ALIGN(4096);
    __loader_end = .;

    /DISCARD/ : {
        *(.comment*)
        *(.gcc_except_table*)
        *(.note*)
        *(.eh_frame*)
        *(.rel.eh_frame*)
    }
}
