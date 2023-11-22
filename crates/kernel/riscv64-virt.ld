OUTPUT_ARCH(riscv)
ENTRY(_start)

KERNEL_OFFSET = 0x80200000;

SECTIONS {
    . = KERNEL_OFFSET;

    .text : {
        __text_start = .;
        *(.text.start)
        *(.text .text.*)
        __text_end = .;
    }

    __global_pointer$ = .;

    .rodata : {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
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
        . = ALIGN(8);
        __bss_end = .;
    }

    .eh_frame : {
        . = ALIGN(8);
        __eh_frame_start = .;
        KEEP (*(.eh_frame)) *(.rel.eh_frame*)
        __eh_frame_end = .;
    }

    . = ALIGN(16);
    __stack_start = .;

    /DISCARD/ : {
        *(.comment*)
        *(.gcc_except_table*)
        *(.note*)
    }
}