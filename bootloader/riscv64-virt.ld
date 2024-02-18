OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY {
    DRAM : ORIGIN = 0x80200000, LENGTH = 8M
}

SECTIONS {
    .text : {
        __text_start = .;
        *(.text.start)
        *(.text .text.*)
        . = ALIGN(4096); # 4KB page alignment for mapping
        __text_end = .;
    } > DRAM

    __global_pointer$ = .;

    .rodata : {
        . = ALIGN(4096); # 4KB page alignment for mapping
        __rodata_start = .;
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        __rodata_end = .;
    } > DRAM

    .eh_frame : {
        __eh_frame_start = .;
        KEEP (*(.eh_frame)) *(.rel.eh_frame*)
        . = ALIGN(4096); # 4KB page alignment for mapping
        __eh_frame_end = .;
    } > DRAM

    .data : {
        . = ALIGN(4096); # 4KB page alignment for mapping
        __data_start = .;
        *(.data .data.*)
        *(.sdata .sdata.*)
        __data_end = .;
    } > DRAM

    .bss (NOLOAD) : {
        . = ALIGN(8); # 8-byte alignment for u64 ptr trick
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        __bss_end = .;
    } > DRAM

    . = ALIGN(4096); # 4KB page alignment for mapping
    __stack_start = .;

    /DISCARD/ : {
        *(.comment*)
        *(.gcc_except_table*)
        *(.note*)
    }
}