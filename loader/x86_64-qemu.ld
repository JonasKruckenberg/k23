OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

KERNEL_OFFSET = 0x100000;

/* Single segment for testing */
PHDRS {
    load PT_LOAD FLAGS(5); /* r-x */
    note PT_NOTE FLAGS(4); /* r-- */
}

SECTIONS {
    . = KERNEL_OFFSET;
    
    /* Put note first so QEMU can find it */
    .note : {
        *(.note.Xen)
        *(.note*)
    } :note
    
    . = ALIGN(4096);

    /* Everything in one segment */
    .text : {
        __text_start = .;
        *(.text.start)
        *(.text .text.*)
        . = ALIGN(16);
        __text_end = .;
    } :load

    .rodata : {
        . = ALIGN(16);
        __rodata_start = .;
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

    __stack_start = .;
}