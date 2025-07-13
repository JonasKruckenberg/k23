// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod stored;

use alloc::boxed::Box;
use core::pin::Pin;
use core::ptr::NonNull;
use pin_project::pin_project;
use stored::StoredData;

use crate::Engine;
use crate::vm::VMStoreContext;
pub use stored::Stored;

#[derive(Debug)]
pub struct Store<T>(Pin<Box<StoreInner<T>>>);

#[derive(Debug)]
#[pin_project]
pub(crate) struct StoreInner<T> {
    #[pin]
    opaque: StoreOpaque,
    data: T,
}

#[pin_project(!Unpin)]
#[derive(Debug)]
pub(crate) struct StoreOpaque {
    /// The engine this store belongs to, used mainly for compatibility checking and to access the
    /// global type registry.
    engine: Engine,
    /// Indexed data within this `Store`, used to store information about
    /// globals, functions, memories, etc.
    ///
    /// Note that this is `ManuallyDrop` because it needs to be dropped before
    /// `rooted_host_funcs` below. This structure contains pointers which are
    /// otherwise kept alive by the `Arc` references in `rooted_host_funcs`.
    stored: StoredData,
    /// Data that is shared across all instances in this store such as stack limits, epoch pointer etc.
    /// This field is accessed by guest code
    vm_store_context: VMStoreContext,
}

// ===== impl Store =====

impl<T> Store<T> {
    pub fn new(engine: Engine, data: T) -> Self {
        let mut inner = Box::new(StoreInner {
            opaque: StoreOpaque {
                engine,
                vm_store_context: VMStoreContext::default(),
                stored: StoredData::default(),
                // alloc,
                // default_caller: InstanceHandle::null(),
                // wasm_vmval_storage: vec![],
                // host_globals: vec![],
                // host_tables: vec![],
                // interpreter: Interpreter::new(),
                // _pinned: PhantomPinned,
            },
            data,
        });

        // inner.opaque.default_caller = {
        //     let mut instance = inner
        //         .opaque
        //         .alloc
        //         .allocate_module(Module::new_stub(engine.clone()))
        //         .expect("failed to allocate default callee");
        //
        //     instance
        //         .instance_mut()
        //         .set_store(Some(NonNull::from(&mut inner.opaque)));
        //
        //     instance
        // };

        Self(Box::into_pin(inner))
    }
}

// ===== impl StoreOpaque =====

impl StoreOpaque {
    #[inline]
    pub(super) fn engine(&self) -> &Engine {
        &self.engine
    }
    #[inline]
    pub(super) fn vm_store_context_ptr(&mut self) -> NonNull<VMStoreContext> {
        NonNull::from(&mut self.vm_store_context)
    }
}
