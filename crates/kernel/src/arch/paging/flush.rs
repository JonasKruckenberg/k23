use crate::paging::VirtualAddress;
use core::mem;

pub struct Flush {
    virt: VirtualAddress,
}

impl Flush {
    pub(crate) fn new(virt: VirtualAddress) -> Self {
        Self { virt }
    }

    pub fn flush(self) {
        // TODO check if this is necessary & make SBI call instead
        unsafe {
            riscv::asm::sfence_vma(0, self.virt.as_raw());
        }
    }
    pub unsafe fn ignore(self) {
        mem::forget(self);
    }
}
