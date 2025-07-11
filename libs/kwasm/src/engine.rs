// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::loom::sync::atomic::AtomicU64;
use crate::loom::sync::Arc;
use crate::type_registry::TypeRegistry;

#[derive(Debug, Clone)]
pub struct Engine(Arc<EngineInner>);

#[derive(Debug)]
struct EngineInner {
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
        Engine(Arc::new(EngineInner {
            type_registry: TypeRegistry::new(),
            epoch_counter: AtomicU64::new(0),
        }))
    }
}

impl Engine {
    pub fn same(lhs: &Engine, rhs: &Engine) -> bool {
        Arc::ptr_eq(&lhs.0, &rhs.0)
    }

    /// Returns the type registry of this engine, used to canonicalize types.
    pub fn type_registry(&self) -> &TypeRegistry {
        &self.0.type_registry
    }

    pub fn epoch_counter(&self) -> &AtomicU64 {
        &self.0.epoch_counter
    }
}
