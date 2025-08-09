// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod stored;

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::alloc::Allocator;
use core::pin::Pin;
use core::ptr::NonNull;

use pin_project::pin_project;
pub use stored::Stored;
use stored::StoredData;

use crate::Engine;
use crate::vm::{InstanceAllocator, InstanceHandle, VMStoreContext, VMVal};

pub struct Store<T>(Pin<Box<StoreInner<T>>>);

#[pin_project]
pub(crate) struct StoreInner<T> {
    #[pin]
    pub(crate) opaque: StoreOpaque,
    data: T,
}

#[derive(Debug)]
#[pin_project(!Unpin)]
pub struct StoreOpaque {
    alloc: InstanceAllocator,
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
    /// The array calling convention requires the first argument to be a `NonNull<VMContext>` pointer
    /// to the function caller. When calling functions from the host though, there is no active VMContext
    /// (we aren't the in VM yet after all) so we allocate a fake instance at store creation that we
    /// can use as the placeholder caller vmctx in these cases. Note that this is easier than special
    /// casing this (e.g. making the caller argument `Option<NonNull<VMContext>>`) since that would require
    /// piping around this option all throughout the code. This fake instance lets us keep all code the
    /// same.
    default_caller: InstanceHandle,
    /// Used to optimized host->wasm calls when calling a function dynamically (through `Func::call`)
    /// to avoid allocating a new vector each time a function is called.
    wasm_vmval_storage: Vec<VMVal>,
}

// ===== impl Store =====

impl<T> Store<T> {
    pub fn new(engine: Engine, alloc: Box<dyn Allocator>, data: T) -> Self {
        let mut inner = Box::new(StoreInner {
            opaque: StoreOpaque {
                engine: engine.clone(),
                vm_store_context: VMStoreContext::default(),
                stored: StoredData::default(),
                default_caller: InstanceHandle::null(),
                wasm_vmval_storage: vec![],
                alloc: InstanceAllocator::new(alloc),
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

    pub fn data(&self) -> &T {
        &self.0.data
    }

    pub fn data_mut(&mut self) -> &mut T {
        self.0.as_mut().project().data
    }

    pub fn into_data(self) -> T {
        let inner = unsafe { Pin::into_inner_unchecked(self.0) };
        inner.data
    }

    pub fn opaque(&self) -> &StoreOpaque {
        &self.0.opaque
    }

    pub fn opaque_mut(&mut self) -> Pin<&mut StoreOpaque> {
        unsafe { Pin::map_unchecked_mut(self.0.as_mut(), |inner| &mut inner.opaque) }
    }
}

// ===== impl StoreOpaque =====

impl StoreOpaque {
    #[inline]
    pub(crate) fn engine(&self) -> &Engine {
        &self.engine
    }
    #[inline]
    pub(crate) fn vm_store_context_ptr(&mut self) -> NonNull<VMStoreContext> {
        NonNull::from(&mut self.vm_store_context)
    }
}
