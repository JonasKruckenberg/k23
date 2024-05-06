use crate::kconfig;
use crate::rt::compile::Compiler;
use core::sync::atomic::{AtomicU64, Ordering};
use cranelift_codegen::isa::OwnedTargetIsa;

/// Globally shared state for runtime
pub struct Engine {
    /// Used to compile individual WASM functions to machine code
    compiler: Compiler,
    /// Used to generate unique IDs for compiled modules
    module_id_allocator: UniqueIdAllocator,
    /// Used to generate unique address space IDs for linear memory instances
    address_space_id_allocator: UniqueIdAllocator,
}

/// Simple struct to hand out globally unique numbers that can be used as identifiers
#[derive(Default)]
struct UniqueIdAllocator {
    next: AtomicU64,
}

impl Engine {
    pub fn new(isa: OwnedTargetIsa) -> Self {
        Self {
            compiler: Compiler::new(isa),
            module_id_allocator: UniqueIdAllocator::default(),
            address_space_id_allocator: UniqueIdAllocator::default(),
        }
    }

    pub fn compiler(&self) -> &Compiler {
        &self.compiler
    }

    pub fn next_address_space_id(&self) -> u64 {
        self.address_space_id_allocator.next()
    }

    pub fn stack_limit(&self) -> usize {
        16 * kconfig::PAGE_SIZE
    }
}

impl UniqueIdAllocator {
    pub fn next(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }
}
