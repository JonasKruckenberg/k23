/*
 * x86_64 has a first-class `x64_64-unknown-uefi` rustc target; rustc drives
 * `lld-link` (MSVC flavor) and produces a PE directly. Section layout, entry
 * point, and output format are all handled by the target — this script only
 * supplements it with the section-boundary symbols that the loader references
 * via `extern "C"` (`__text_start`, `__bss_start`, etc.).
 */

SECTIONS {
    .text : {
        __text_start = .;
        *(.text .text.*)
        . = ALIGN(4096);
        __text_end = .;
    }

    .rodata : {
        . = ALIGN(4096);
        __rodata_start = .;
        *(.got .got.*)
        *(.rodata .rodata.*)
        . = ALIGN(4096);
        __rodata_end = .;
    }

    .data : {
        . = ALIGN(4096);
        __data_start = .;
        *(.data .data.*)
        . = ALIGN(4096);
        __data_end = .;
    }

    .bss (NOLOAD) : {
        . = ALIGN(4096);
        __bss_start = .;
        *(.bss.uninit)
        . = ALIGN(8);
        __bss_zero_start = .;
        *(.bss .bss.*)
        __bss_end = .;
    }

    __stack_start = .;
}
