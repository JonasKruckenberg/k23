OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY {
    DRAM (wxa) : ORIGIN = 0x80200000, LENGTH = 64M
}

SECTIONS {
    .text : {
        *(.text.start)
        *(.text .text.*)
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

    /DISCARD/ : {
        *(.comment*)
        *(.eh_frame*)
        *(.gcc_except_table*)
        *(.note*)
        *(.rel.eh_frame*)
    }
}