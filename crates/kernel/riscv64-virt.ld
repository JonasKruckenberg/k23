OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY {
    DRAM (wxa) : ORIGIN = 0x80200000, LENGTH = 64M
}

SECTIONS {
    .text : {
        __text_start = .;
        *(.text.start)
        *(.text .text.*)
        __text_end = .;
    } > DRAM

    __global_pointer$ = .;

    .rodata : {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    } > DRAM

    .data : {
        *(.data .data.*)
        *(.sdata .sdata.*)
    } > DRAM

    .bss (NOLOAD) : {
        . = ALIGN(8);
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        . = ALIGN(8);
        __bss_end = .;
    } > DRAM

    .eh_frame : {
        . = ALIGN(8);
        __eh_frame_start = .;
        KEEP (*(.eh_frame)) *(.rel.eh_frame*)
        __eh_frame_end = .;
    } > DRAM

    . = ALIGN(16);
    __stack_start = .;

    /DISCARD/ : {
        *(.comment*)
        *(.gcc_except_table*)
        *(.note*)
    }
}