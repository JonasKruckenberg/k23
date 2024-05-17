use crate::rt::codegen::{compile_module, CompiledModuleInfo};
use crate::rt::engine::Engine;
use crate::rt::guest_memory::{AlignedVec, CodeMemory};
use crate::rt::store::Store;
use crate::rt::vmcontext::VMContextPlan;
use alloc::sync::Arc;

#[derive(Debug)]
pub struct Module<'wasm> {
    pub info: Arc<CompiledModuleInfo<'wasm>>,
    pub code: Arc<CodeMemory>,
    pub vmctx_plan: VMContextPlan,
}

impl<'wasm> Module<'wasm> {
    pub fn from_binary(engine: &Engine, store: &Store, bytes: &'wasm [u8]) -> Self {
        let mut guest_vec = AlignedVec::new(store.guest_allocator());
        let info = compile_module(engine, bytes, &mut guest_vec).unwrap();
        log::trace!("compile output {:?}", guest_vec.as_ptr_range());

        let mut code = CodeMemory::new(guest_vec);
        code.publish().unwrap();

        Self {
            vmctx_plan: VMContextPlan::for_module(engine.compiler().target_isa(), &info.module),
            info: Arc::new(info),
            code: Arc::new(code),
        }
    }
}
