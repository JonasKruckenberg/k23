pub unsafe fn disable() {
    riscv::register::sstatus::clear_sie();
}

pub unsafe fn enable() {
    riscv::register::sstatus::set_sie();
}

pub fn without<R>(f: impl FnOnce() -> R) -> R {
    let status = riscv::register::sstatus::read();

    unsafe {
        disable();
    }

    let r = f();

    if status.sie() {
        unsafe {
            enable();
        }
    }

    r
}
