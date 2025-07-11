// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod stored;

use alloc::boxed::Box;
use core::marker::PhantomPinned;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use pin_project::pin_project;
use stored::StoredData;

pub use stored::Stored;
use crate::Engine;

#[derive(Debug)]
pub struct Store<T>(Pin<Box<StoreInner<T>>>);

#[derive(Debug)]
#[pin_project]
struct StoreInner<T> {
    #[pin]
    opaque: StoreOpaque,
    data: T,
}

#[pin_project(!Unpin)]
#[derive(Debug)]
struct StoreOpaque {
    engine: Engine,
    stored: StoredData,
}

// ===== impl Store =====

impl<T> Store<T> {
    pub fn new(engine: Engine, data: T) -> Self {
        let mut inner = Box::new(StoreInner {
            opaque: StoreOpaque {
                engine,
                // alloc,
                // vm_store_context: VMStoreContext::default(),
                stored: StoredData::default(),
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
}