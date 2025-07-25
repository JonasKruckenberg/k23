// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::sync::Arc;
use core::sync::atomic::AtomicU64;

use cranelift_codegen::settings::{Configurable, Flags};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use spin::{Mutex, MutexGuard};

use crate::arch;
use crate::wasm::compile::Compiler;
use crate::wasm::cranelift::CraneliftCompiler;
use crate::wasm::type_registry::TypeRegistry;

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
    rng: Option<Mutex<ChaCha20Rng>>,
    asid_alloc: Mutex<arch::AsidAllocator>,
    epoch_counter: AtomicU64,
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
            rng: None,
            asid_alloc: Mutex::new(arch::AsidAllocator::new()),
            epoch_counter: AtomicU64::new(0),
        }))
    }
}

impl Engine {
    pub fn new(rng: &mut impl Rng) -> Self {
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
            rng: Some(Mutex::new(ChaCha20Rng::from_rng(rng))),
            asid_alloc: Mutex::new(arch::AsidAllocator::new()),
            epoch_counter: AtomicU64::new(0),
        }))
    }

    pub fn same(lhs: &Engine, rhs: &Engine) -> bool {
        Arc::ptr_eq(&lhs.0, &rhs.0)
    }

    pub fn compiler(&self) -> &dyn Compiler {
        &self.0.compiler
    }

    /// Returns the type registry of this engine, used to canonicalize types.
    pub fn type_registry(&self) -> &TypeRegistry {
        &self.0.type_registry
    }

    pub fn allocate_asid(&self) -> u16 {
        let mut alloc = self.0.asid_alloc.lock();
        alloc.alloc().expect("out of address space identifiers")
    }
    pub fn rng(&self) -> Option<MutexGuard<'_, ChaCha20Rng>> {
        Some(self.0.rng.as_ref()?.lock())
    }
    pub fn epoch_counter(&self) -> &AtomicU64 {
        &self.0.epoch_counter
    }
}
