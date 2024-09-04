OUTPUT_ARCH(riscv)
ENTRY(_start)

LOADER_OFFSET = 0x80200000;

SECTIONS {
    . = LOADER_OFFSET;
    . = ALIGN(4096);

    .text : {
        __text_start = .;
        *(.text.start)
        *(.text .text.*)
        . = ALIGN(4096); # 4KB page alignment for mapping
        __text_end = .;
    }

    .rodata : {
        . = ALIGN(4096); # 4KB page alignment for mapping
        __rodata_start = .;
        *(.got .got.*)
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        . = ALIGN(4096); # 4KB page alignment for mapping
        __rodata_end = .;
    }

    .bss (NOLOAD) : {
        . = ALIGN(8); # 8-byte alignment for u64 ptr trick
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        __bss_end = .;
    }

    .data : {
        __data_start = .;
        PROVIDE(__global_pointer$ = . + 0x800);
        *(.data .data.*)
        *(.sdata .sdata.*)
        . = ALIGN(4096); # 4KB page alignment for mapping
        __data_end = .;
    }

    . = ALIGN(4096);
    __stack_start = .;

    /DISCARD/ : {
        *(.comment*)
        *(.gcc_except_table*)
        *(.note*)
        *(.eh_frame*)
        *(.rel.eh_frame*)
    }
}
