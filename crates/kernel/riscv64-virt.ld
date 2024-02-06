OUTPUT_ARCH(riscv)
ENTRY(_start)

KERNEL_OFFSET = 0x80200000;

SECTIONS {
    . = KERNEL_OFFSET;
    . = ALIGN(4096);

    .text : {
        __text_start = .;
        *(.text.start)
        *(.text .text.*)
        . = ALIGN(4096);
        __text_end = .;
    }

    __global_pointer$ = .;

    .rodata : {
        __rodata_start = .;
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        . = ALIGN(4096);
        __rodata_end = .;
    }

    .eh_frame : {
        . = ALIGN(8);
        __eh_frame_start = .;
        KEEP (*(.eh_frame)) *(.rel.eh_frame*)
        __eh_frame_end = .;
    }

    .data : {
        *(.data .data.*)
        *(.sdata .sdata.*)
    }

    .bss (NOLOAD) : {
        . = ALIGN(8);
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        . = ALIGN(4096);
        __bss_end = .;
    }

    . = ALIGN(4096);
    __stack_start = .;

    /DISCARD/ : {
        *(.comment*)
        *(.gcc_except_table*)
        *(.note*)
    }
}