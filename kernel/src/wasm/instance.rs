// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::module::Module;
use crate::wasm::store::{StoreOpaque, Stored};
use crate::wasm::vm;
use crate::wasm::vm::{ConstExprEvaluator, Imports, InstanceHandle};

/// An instantiated WebAssembly module.
///
/// This is the main representation of all runtime state associated with a running WebAssembly module.
///
/// # Instance and `VMContext`
///
/// `Instance` and `VMContext` are essentially two halves of the same data structure. `Instance` is
/// the privileged host-side half responsible for administrating execution, while `VMContext` holds the
/// actual data that is accessed by compiled WASM code.
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct Instance(Stored<InstanceHandle>);

impl Instance {
    /// Instantiates a new `Instance`.
    ///
    /// # Safety
    ///
    /// This functions assumes the provided `imports` have already been validated and typechecked for
    /// compatibility with the `module` being instantiated.
    pub(crate) unsafe fn new_unchecked(
        store: &mut StoreOpaque,
        const_eval: &mut ConstExprEvaluator,
        module: Module,
        imports: Imports,
    ) -> crate::Result<Self> {
        // Safety: caller has to ensure safety
        let handle =
            unsafe { vm::Instance::new_unchecked(store, const_eval, module, imports)? };
        let stored = store.add_instance(handle);
        Ok(Self(stored))
    }
}