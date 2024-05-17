mod builtins;
mod codegen;
mod const_expr;
mod engine;
mod errors;
mod guest_memory;
mod instance;
mod linker;
mod memory;
mod module;
mod store;
mod table;
mod trap;
mod utils;
mod vmcontext;

pub use engine::Engine;
pub use instance::Instance;
pub use linker::Linker;
pub use module::Module;
pub use store::Store;

/// Namespace corresponding to wasm functions, the index is the index of the
/// defined function that's being referenced.
pub const NS_WASM_FUNC: u32 = 0;

/// Namespace for builtin function trampolines. The index is the index of the
/// builtin that's being referenced.
pub const NS_WASM_BUILTIN: u32 = 1;

/// WebAssembly page sizes are defined to be 64KiB.
pub const WASM_PAGE_SIZE: u32 = 0x10000;

/// The number of pages (for 32-bit modules) we can have before we run out of
/// byte index space.
pub const WASM32_MAX_PAGES: u64 = 1 << 16;
/// The number of pages (for 64-bit modules) we can have before we run out of
/// byte index space.
pub const WASM64_MAX_PAGES: u64 = 1 << 48;

// pub struct Stack {
//     inner: GuestVec<u8, 16>,
// }
//
// impl Stack {
//     pub fn new(stack_size: usize, alloc: GuestAllocator) -> Result<Self, ()> {
//         let mut inner = GuestVec::try_with_capacity(stack_size, alloc)?;
//         inner.try_resize(stack_size, 0)?;
//
//         Ok(Self { inner })
//     }
//
//     pub fn stack_ptr(&mut self) -> *mut u8 {
//         self.inner.as_mut_ptr_range().end
//     }
//
//     pub fn stack_limit(&mut self) -> *const u8 {
//         self.inner.as_ptr_range().start
//     }
//
//     pub unsafe fn on_stack(
//         &mut self,
//         vmctx: *mut VMContext,
//         func: unsafe extern "C" fn(*mut VMContext, usize),
//     ) {
//         log::trace!("switching to stack {:?}", self.inner.as_ptr_range());
//
//         let arg: usize = 7;
//         let ret: usize;
//         asm!(
//         "csrrw sp, sscratch, sp",
//
//         "mv  sp, {wasm_stack_ptr}",
//         "mv  a0, {vmctx_ptr}",
//
//         "mv  a1, {arg}",
//         "jalr {func}",
//
//         "csrrw sp, sscratch, sp",
//         wasm_stack_ptr = in(reg) self.stack_ptr(),
//         vmctx_ptr = in(reg) vmctx,
//         func = in(reg) func,
//         arg = in(reg) arg - 2,
//         out("a0") ret
//         );
//         log::trace!(
//             r#"
//
// WASM says: The {}th fibonacci number is {ret:?}!
//
// "#,
//             arg
//         );
//         log::trace!("switched back from stack {:?}", self.inner.as_ptr_range());
//     }
// }
//
// impl fmt::Debug for Stack {
//     fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
//         f.debug_struct("Stack")
//             .field("address_range", &self.inner.as_ptr_range())
//             .finish()
//     }
// }
