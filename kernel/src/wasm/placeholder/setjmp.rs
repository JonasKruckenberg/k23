//! definitions for `jmp_buf`, `setjmp` and `longjmp`
#![expect(non_camel_case_types, reason = "FFI types")]
include!(concat!(env!("OUT_DIR"), "/setjmp.rs"));
