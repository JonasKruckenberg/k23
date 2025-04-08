// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod stored;

use crate::wasm::module_registry::ModuleRegistry;
use crate::wasm::vm::{InstanceAllocator, InstanceHandle, VMContext, VMStoreContext, VMVal};
use crate::wasm::{Engine, Module};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::marker::PhantomPinned;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::ptr::NonNull;
use core::{fmt, mem};
use pin_project::pin_project;
use static_assertions::assert_impl_all;

pub use stored::{Stored, StoredData};

pub struct Store<T>(Pin<Box<StoreInner<T>>>);

#[repr(C)]
#[pin_project]
pub(super) struct StoreInner<T> {
    pub(super) opaque: StoreOpaque,
    pub(super) data: T,
}

impl<T> Store<T> {
    pub fn new(
        engine: &Engine,
        alloc: Box<dyn InstanceAllocator + Send + Sync>,
        data: T,
    ) -> Self {
        let mut inner = Box::new(StoreInner {
            opaque: StoreOpaque {
                engine: engine.clone(),
                alloc,
                vm_store_context: VMStoreContext::default(),
                modules: ModuleRegistry::default(),
                stored: StoredData::default(),
                default_caller: InstanceHandle::null(),
                wasm_vmval_storage: vec![],
                _m: PhantomPinned,
            },
            data,
        });
        
        inner.opaque.default_caller = {
            let mut instance = inner
                .opaque
                .alloc
                .allocate_module(Module::new_stub(engine.clone()))
                .expect("failed to allocate default callee");
        
            unsafe {
                instance
                    .instance_mut()
                    .set_store(Some(NonNull::from(&mut inner.opaque)));
            }
        
            instance
        };
        
        Self(Box::into_pin(inner))
    }
}

impl<T> Deref for Store<T> {
    type Target = StoreOpaque;

    fn deref(&self) -> &Self::Target {
        &self.0.opaque
    }
}

impl<T> DerefMut for Store<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0.opaque
    }
}

pub struct StoreOpaque {
    /// The engine this store belongs to, used mainly for compatability checking and to access the
    /// global type registry.
    engine: Engine,
    /// The instance allocator that manages all the runtime memory for Wasm instances.
    alloc: Box<dyn InstanceAllocator + Send + Sync>,
    /// Data that is shared across all instances in this store such as stack limits, epoch pointer etc.
    /// This field is accessed by guest code
    vm_store_context: VMStoreContext,
    /// Modules in use with this store.
    ///
    /// Modules are independently reference counted, so we use this field to store a reference to each
    /// module that was instantiated in this store to make sure they are not freed as long as this store
    /// is around.
    modules: ModuleRegistry,
    /// Indexed data within this `Store`, used to store information about
    /// globals, functions, memories, etc.
    ///
    /// Note that this is `ManuallyDrop` because it needs to be dropped before
    /// `rooted_host_funcs` below. This structure contains pointers which are
    /// otherwise kept alive by the `Arc` references in `rooted_host_funcs`.
    stored: StoredData,
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

    _m: PhantomPinned,
}
assert_impl_all!(StoreOpaque: Send, Sync);

impl fmt::Debug for StoreOpaque {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreOpaque").finish_non_exhaustive()
    }
}

impl StoreOpaque {
    #[inline]
    pub(super) fn engine(&self) -> &Engine {
        &self.engine
    }
    #[inline]
    pub(super) fn alloc_mut(&mut self) -> &mut (dyn InstanceAllocator + Send + Sync) {
        self.alloc.as_mut()
    }
    #[inline]
    pub(super) fn vm_store_context_ptr(&self) -> NonNull<VMStoreContext> {
        NonNull::from(&self.vm_store_context)
    }
    #[inline]
    pub(super) fn modules_mut(&mut self) -> &mut ModuleRegistry {
        &mut self.modules
    }
    #[inline]
    pub(super) fn default_caller(&self) -> NonNull<VMContext> {
        self.default_caller.vmctx()
    }

    /// Takes the `Vec<VMVal>` storage used for passing arguments using the array call convention.
    #[inline]
    pub(super) fn take_wasm_vmval_storage(&mut self) -> Vec<VMVal> {
        mem::take(&mut self.wasm_vmval_storage)
    }
    /// Returns the `Vec<VMVal>` storage allowing it's allocation to be reused for the next array call.
    #[inline]
    pub(super) fn return_wasm_vmval_storage(&mut self, storage: Vec<VMVal>) {
        self.wasm_vmval_storage = storage;
    }
}

// use crate::wasm::module_registry::ModuleRegistry;
// use crate::wasm::vm::{InstanceAllocator, InstanceHandle, VMContext, VMStoreContext, VMVal};
// use crate::wasm::{Engine, Module};
// use alloc::boxed::Box;
// use alloc::vec;
// use alloc::vec::Vec;
// use core::marker::PhantomPinned;
// use core::mem::ManuallyDrop;
// use core::ptr::NonNull;
// use core::{fmt, mem};
// use core::ops::{Deref, DerefMut};
// use static_assertions::assert_impl_all;

// pub struct Store<T>(ManuallyDrop<Box<StoreInner<T>>>);
// pub(super) struct StoreInner<T> {
//     pub opaque: StoreOpaque,
//     pub data: T,
// }

//
//

//
// impl<T> Deref for Store<T> {
//     type Target = StoreInner<T>;
//
//     fn deref(&self) -> &Self::Target {
//         self.0.as_ref()
//     }
// }
// impl<T> DerefMut for Store<T> {
//     fn deref_mut(&mut self) -> &mut Self::Target {
//         self.0.as_mut()
//     }
// }
// impl<T> Deref for StoreInner<T> {
//     type Target = StoreOpaque;
//
//     fn deref(&self) -> &Self::Target {
//         &self.opaque
//     }
// }
// impl<T> DerefMut for StoreInner<T> {
//     fn deref_mut(&mut self) -> &mut Self::Target {
//         &mut self.opaque
//     }
// }
//
// impl<T> Drop for Store<T> {
//     fn drop(&mut self) {
//         // self.inner.flush_fiber_stack();
//
//         // for documentation on this `unsafe`, see `into_data`.
//         unsafe {
//             ManuallyDrop::drop(&mut self.0.data);
//         }
//     }
// }
//
// impl Drop for StoreOpaque {
//     fn drop(&mut self) {
//         unsafe {
//             for data in self.stored.instances.iter_mut() {
//                 self.alloc.deallocate_module(&mut data.handle);
//             }
//
//             ManuallyDrop::drop(&mut self.stored);
//             // ManuallyDrop::drop(&mut self.rooted_host_funcs);
//         }
//     }
// }
