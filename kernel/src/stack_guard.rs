//! Supporting code for the `stack-protector` Rust/LLVM feature

#[no_mangle]
pub static mut __stack_chk_guard: u64 = 0xACE0BACE;

#[no_mangle]
pub unsafe extern "C" fn __stack_chk_fail() {
    panic!("Kernel stack is corrupted")
}
