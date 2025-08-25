.global _start
.extern _rust_start

.section .text
_start:
    # The loader passes:
    # rdi = cpuid
    # rsi = boot_info  
    # rdx = boot_ticks
    # Just pass them through to Rust
    call _rust_start
    # Should never return
    ud2