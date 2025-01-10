use crate::wasm::compile::Compiler;
use crate::wasm::cranelift::CraneliftCompiler;
use crate::wasm::type_registry::TypeRegistry;
use alloc::sync::Arc;
use cranelift_codegen::settings::{Configurable, Flags};

/// Global context for the runtime.
///
/// An engine can be safely shared across threads and is a cheap cloneable
/// handle to the actual engine. The engine itself will be deallocated once all
/// references to it have gone away.
#[derive(Debug, Clone)]
pub struct Engine(Arc<EngineInner>);

#[derive(Debug)]
pub struct EngineInner {
    compiler: CraneliftCompiler,
    type_registry: TypeRegistry,
}

impl Default for Engine {
    fn default() -> Self {
        let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
        let mut b = cranelift_codegen::settings::builder();
        b.set("opt_level", "speed_and_size").unwrap();
        b.set("libcall_call_conv", "isa_default").unwrap();
        b.set("preserve_frame_pointers", "true").unwrap();
        b.set("enable_probestack", "true").unwrap();
        b.set("probestack_strategy", "inline").unwrap();
        let target_isa = isa_builder.finish(Flags::new(b)).unwrap();

        Self(Arc::new(EngineInner {
            compiler: CraneliftCompiler::new(target_isa),
            type_registry: TypeRegistry::default(),
        }))
    }
}

impl Engine {
    pub(crate) fn compiler(&self) -> &dyn Compiler {
        &self.0.compiler
    }

    /// Returns the type registry of this engine, used to canonicalize types.
    pub fn type_registry(&self) -> &TypeRegistry {
        &self.0.type_registry
    }

    pub(crate) fn same(lhs: &Engine, rhs: &Engine) -> bool {
        Arc::ptr_eq(&lhs.0, &rhs.0)
    }
}
