use crate::runtime::instantiate::{GuestAllocator, GuestVec};
use crate::runtime::VMContext;
use core::arch::asm;
use core::fmt;
use core::fmt::Formatter;

pub struct Stack {
    inner: GuestVec<u8, 16>,
}

impl Stack {
    pub fn new(stack_size: usize, alloc: GuestAllocator) -> Result<Self, ()> {
        let mut inner = GuestVec::try_with_capacity(stack_size, alloc)?;
        inner.try_resize(stack_size, 0)?;

        Ok(Self { inner })
    }

    pub fn stack_ptr(&mut self) -> *mut u8 {
        self.inner.as_mut_ptr_range().end
    }

    pub fn stack_limit(&mut self) -> *const u8 {
        self.inner.as_ptr_range().start
    }

    pub unsafe fn on_stack(
        &mut self,
        vmctx: *mut VMContext,
        func: unsafe extern "C" fn(*mut VMContext, usize),
    ) {
        log::trace!("switching to stack {:?}", self.inner.as_ptr_range());

        let arg: usize = 6;
        let ret: usize;
        asm!(
            "csrrw sp, sscratch, sp",

            "mv  sp, {wasm_stack_ptr}",
            "mv  a0, {vmctx_ptr}",

            "mv  a1, {arg}",
            "jalr {func}",

            "csrrw sp, sscratch, sp",
            wasm_stack_ptr = in(reg) self.stack_ptr(),
            vmctx_ptr = in(reg) vmctx,
            func = in(reg) func,
            arg = in(reg) arg,
            out("a0") ret
        );
        log::trace!(
            r#"










WASM says: The {}th fibonacci number is {ret:?}!










"#,
            arg + 1
        );
        log::trace!("switched back from stack {:?}", self.inner.as_ptr_range());
    }
}

impl fmt::Debug for Stack {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stack")
            .field("address_range", &self.inner.as_ptr_range())
            .finish()
    }
}
