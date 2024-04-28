OUTPUT_ARCH(riscv)
ENTRY(kstart)

KERNEL_OFFSET = 0xffffffff80000000;

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
        __rodata_start = .;
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
        . = ALIGN(4096); # 4KB page alignment for mapping
        __bss_end = .;
    }

    .data : {
        __data_start = .;
        *(.data .data.*)
        *(.sdata .sdata.*)
        . = ALIGN(4096); # 4KB page alignment for mapping
        __data_end = .;
    }

    .tbss : {
         __tbss_start = .;
         *(.tbss .tbss.*)
         __tbss_end = .;
    }

    .tdata : {
         __tdata_start = .;
         *(.tdata .tdata.*)
         __tdata_end = .;
    }

    /DISCARD/ : {
        *(.comment*)
        *(.gcc_except_table*)
        *(.note*)
        *(.eh_frame*)
        *(.rel.eh_frame*)
    }
}
