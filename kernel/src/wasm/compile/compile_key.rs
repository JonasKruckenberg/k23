use crate::wasm::builtins::BuiltinFunctionIndex;
use crate::wasm::indices::{DefinedFuncIndex, ModuleInternedTypeIndex};

/// A sortable, comparable key for a compilation output.
/// This is used to sort by compilation output kind and bucket results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompileKey {
    // The namespace field is bitpacked like:
    //
    //     [ kind:i3 module:i29 ]
    namespace: u32,
    pub index: u32,
}

impl CompileKey {
    const KIND_BITS: u32 = 3;
    const KIND_OFFSET: u32 = 32 - Self::KIND_BITS;
    const KIND_MASK: u32 = ((1 << Self::KIND_BITS) - 1) << Self::KIND_OFFSET;

    pub fn kind(self) -> u32 {
        self.namespace & Self::KIND_MASK
    }

    pub const WASM_FUNCTION_KIND: u32 = Self::new_kind(0);
    pub const ARRAY_TO_WASM_TRAMPOLINE_KIND: u32 = Self::new_kind(2);
    pub const WASM_TO_ARRAY_TRAMPOLINE_KIND: u32 = Self::new_kind(3);
    pub const WASM_TO_BUILTIN_TRAMPOLINE_KIND: u32 = Self::new_kind(4);

    const fn new_kind(kind: u32) -> u32 {
        assert!(kind < (1 << Self::KIND_BITS));
        kind << Self::KIND_OFFSET
    }

    pub fn wasm_function(index: DefinedFuncIndex) -> Self {
        let module = 0; // TODO change this when we support multiple modules per compilation (components?)
        Self {
            namespace: Self::WASM_FUNCTION_KIND | module,
            index: index.as_u32(),
        }
    }

    pub fn array_to_wasm_trampoline(index: DefinedFuncIndex) -> Self {
        let module = 0; // TODO change this when we support multiple modules per compilation (components?)
        Self {
            namespace: Self::ARRAY_TO_WASM_TRAMPOLINE_KIND | module,
            index: index.as_u32(),
        }
    }

    pub fn wasm_to_array_trampoline(index: ModuleInternedTypeIndex) -> Self {
        let module = 0; // TODO change this when we support multiple modules per compilation (components?)
        Self {
            namespace: Self::WASM_TO_ARRAY_TRAMPOLINE_KIND | module,
            index: index.as_u32(),
        }
    }

    pub fn wasm_to_builtin_trampoline(index: BuiltinFunctionIndex) -> Self {
        Self {
            namespace: Self::WASM_TO_BUILTIN_TRAMPOLINE_KIND,
            index: index.as_u32(),
        }
    }
}
