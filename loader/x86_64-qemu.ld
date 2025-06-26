OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

/* Standard x86_64 kernel load address options:
 * 0x100000 (1MB) - Legacy/Multiboot compatible
 * 0x1000000 (16MB) - Common for modern kernels
 * 0xFFFFFFFF80000000 - Higher half kernel space
 */
KERNEL_OFFSET = 0x100000;

SECTIONS {
    . = KERNEL_OFFSET;
    
    /* Ensure we start on a page boundary */
    . = ALIGN(4096);

    /* Code section - read-only, executable */
    .text : {
        __text_start = .;
        *(.text.start)      /* Entry point code first */
        *(.text .text.*)
        *(.plt .plt.*)      /* Procedure Linkage Table */
        . = ALIGN(4096);
        __text_end = .;
    }

    /* Read-only data */
    .rodata : {
        . = ALIGN(4096);
        __rodata_start = .;
        *(.rodata .rodata.*)
        *(.rodata.str1.*)   /* String constants */
        . = ALIGN(4096);
        __rodata_end = .;
    }

    /* Exception handling and notes */
    .eh_frame : {
        *(.eh_frame)
        *(.eh_frame_hdr)
    }

    /* Notes section - must be in a PT_NOTE segment */
    .note : {
        *(.note.gnu.build-id)
        *(.note.Xen)        /* PVH note */
        *(.note*)
    } :note

    /* Global Offset Table */
    .got : {
        *(.got)
        *(.got.*)
    }

    .got.plt : {
        *(.got.plt)
    }

    /* Initialized data */
    .data : {
        . = ALIGN(4096);
        __data_start = .;
        *(.data .data.*)
        *(.sdata .sdata.*)  /* Small data section */
        . = ALIGN(4096);
        __data_end = .;
    }

    /* Uninitialized data */
    .bss (NOLOAD) : {
        . = ALIGN(4096);
        __bss_start = .;
        
        /* Special uninitialized section (not zeroed) */
        *(.bss.uninit)
        
        . = ALIGN(16);      /* Better alignment for x86_64 */
        __bss_zero_start = .;
        
        *(.bss .bss.*)
        *(.sbss .sbss.*)    /* Small BSS */
        *(COMMON)           /* Common symbols */
        
        . = ALIGN(4096);
        __bss_end = .;
    }

    /* Stack grows downward from here */
    . = ALIGN(4096);
    __stack_start = .;
    
    /* Kernel end marker */
    __kernel_end = .;

    /* Sections to discard */
    /DISCARD/ : {
        *(.comment)
        *(.note.GNU-stack)  /* Stack executable bit info */
        *(.gnu.hash)
        *(.dynsym)
        *(.dynstr)
        *(.dynamic)
        *(.interp)
        *(.hash)
    }
}