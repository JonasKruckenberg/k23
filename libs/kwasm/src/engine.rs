// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use cranelift_codegen::settings::{Configurable, Flags};

use crate::compile::Compiler;
use crate::cranelift::CraneliftCompiler;
use crate::loom::sync::Arc;
use crate::loom::sync::atomic::AtomicU64;
use crate::type_registry::TypeRegistry;

#[derive(Debug, Clone)]
pub struct Engine(Arc<EngineInner>);

#[derive(Debug)]
struct EngineInner {
    compiler: CraneliftCompiler,
    type_registry: TypeRegistry,
    epoch_counter: AtomicU64,
}

// ===== impl Engine =====

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Engine {
        let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
        let mut b = cranelift_codegen::settings::builder();
        b.set("opt_level", "speed_and_size").unwrap();
        b.set("libcall_call_conv", "isa_default").unwrap();
        b.set("preserve_frame_pointers", "true").unwrap();
        b.set("enable_probestack", "true").unwrap();
        b.set("probestack_strategy", "inline").unwrap();
        let target_isa = isa_builder.finish(Flags::new(b)).unwrap();

        Engine(Arc::new(EngineInner {
            compiler: CraneliftCompiler::new(target_isa),
            type_registry: TypeRegistry::new(),
            epoch_counter: AtomicU64::new(0),
        }))
    }
}

impl Engine {
    pub fn same(lhs: &Engine, rhs: &Engine) -> bool {
        Arc::ptr_eq(&lhs.0, &rhs.0)
    }

    pub(crate) fn compiler(&self) -> &dyn Compiler {
        &self.0.compiler
    }

    /// Returns the type registry of this engine, used to canonicalize types.
    pub(crate) fn type_registry(&self) -> &TypeRegistry {
        &self.0.type_registry
    }

    pub(crate) fn epoch_counter(&self) -> &AtomicU64 {
        &self.0.epoch_counter
    }
}
