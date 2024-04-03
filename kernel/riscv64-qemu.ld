OUTPUT_ARCH(riscv)
ENTRY(_start)

KERNEL_OFFSET = 0xffffffff80000000;
#KERNEL_OFFSET = 0x80200000;

SECTIONS {
    . = KERNEL_OFFSET;
    . = ALIGN(4096);

    .text : {
        __text_start = .;
        *(.text.start)
        *(.text .text.*)
        . = ALIGN(4096); # 4KB page alignment for mapping
        __text_end = .;
    }

    __global_pointer$ = .;

    .rodata : {
        . = ALIGN(4096); # 4KB page alignment for mapping
        __rodata_start = .;
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        . = ALIGN(4096);
        __rodata_end = .;
    }

    .data : {
        . = ALIGN(4096); # 4KB page alignment for mapping
        __data_start = .;
        *(.data .data.*)
        *(.sdata .sdata.*)
        __data_end = .;
    }

    .bss (NOLOAD) : {
        . = ALIGN(8); # 8-byte alignment for u64 ptr trick
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        __bss_end = .;
    }

    . = ALIGN(4096); # 4KB page alignment for mapping
    __stack_start = .;

    /DISCARD/ : {
        *(.comment*)
        *(.gcc_except_table*)
        *(.note*)
        *(.eh_frame*)
        *(.rel.eh_frame*)
    }
}
