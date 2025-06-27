OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

KERNEL_OFFSET = 0x100000;

PHDRS {
    load PT_LOAD;
    note PT_NOTE;
}

SECTIONS {
    . = KERNEL_OFFSET;

    .text : {
        . = ALIGN(16);
        __text_start = .;
        *(.text.start)
        *(.text .text.*)
        . = ALIGN(4096);
        __text_end = .;
    } :load

    .note : {
        *(.note.Xen)
        *(.note*)
    } :note

    .rodata : {
        . = ALIGN(16);
        __rodata_start = .;
        *(.got.plt)
        *(.plt .plt.*)
        . = ALIGN(0x1000);
        *(.rodata.kernel)
        *(.got .got.*)
        *(.rodata .rodata.*)
        . = ALIGN(16);
        __rodata_end = .;
    } :load

    .data : {
        . = ALIGN(16);
        __data_start = .;
        *(.data .data.*)
        . = ALIGN(16);
        __data_end = .;
    } :load

    .bss (NOLOAD) : {
        . = ALIGN(16);
        __bss_start = .;
        *(.bss.uninit)
        . = ALIGN(16);
        __bss_zero_start = .;
        *(.bss .bss.*)
        . = ALIGN(4096);
        __bss_end = .;
    } :load

    . = ALIGN(4096);
    __stack_start = .;

    __kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame*)
        *(.comment)
    }
}