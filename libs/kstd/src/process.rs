use crate::arch;

pub fn exit(code: i32) -> ! {
    arch::abort_internal(code)
}
