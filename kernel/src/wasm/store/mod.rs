// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod stored;

use crate::wasm::vm::{InstanceAllocator, InstanceHandle, VMContext, VMFuncRef, VMGlobalDefinition, VMStoreContext, VMVal};
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
use static_assertions::{assert_impl_all, const_assert};

use crate::arch;
use crate::mem::VirtualAddress;
pub use stored::{Stored, StoredData};
use crate::wasm::trap_handler::WasmFault;

pub struct Store<T>(Pin<Box<StoreInner<T>>>);

#[repr(C)]
#[pin_project]
pub(super) struct StoreInner<T> {
    pub(super) opaque: StoreOpaque,
    pub(super) data: T,
}

impl<T> Store<T> {
    pub fn new(engine: &Engine, alloc: Box<dyn InstanceAllocator + Send + Sync>, data: T) -> Self {
        let mut inner = Box::new(StoreInner {
            opaque: StoreOpaque {
                engine: engine.clone(),
                alloc,
                vm_store_context: VMStoreContext::default(),
                stored: StoredData::default(),
                default_caller: InstanceHandle::null(),
                wasm_vmval_storage: vec![],
                host_globals: vec![],
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
    
    host_globals: Vec<VMGlobalDefinition>,

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
    pub(super) fn vm_store_context(&self) -> &VMStoreContext {
        &self.vm_store_context
    }
    #[inline]
    pub(super) fn vm_store_context_ptr(&mut self) -> NonNull<VMStoreContext> {
        NonNull::from(&mut self.vm_store_context)
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

    #[inline]
    pub(super) fn add_host_global(&mut self, def: VMGlobalDefinition) -> NonNull<VMGlobalDefinition> {
        self.host_globals.push(def);
        NonNull::from(self.host_globals.last_mut().unwrap())
    }

    pub(super) fn wasm_fault(
        &self,
        pc: VirtualAddress,
        faulting_addr: VirtualAddress,
    ) -> Option<WasmFault> {
        // There are a few instances where a "close to zero" pointer is loaded
        // and we expect that to happen:
        //
        // * Explicitly bounds-checked memories with spectre-guards enabled will
        //   cause out-of-bounds accesses to get routed to address 0, so allow
        //   wasm instructions to fault on the null address.
        // * `call_indirect` when invoking a null function pointer may load data
        //   from the a `VMFuncRef` whose address is null, meaning any field of
        //   `VMFuncRef` could be the address of the fault.
        //
        // In these situations where the address is so small it won't be in any
        // instance, so skip the checks below.
        if faulting_addr.get() <= size_of::<VMFuncRef>() {
            // static-assert that `VMFuncRef` isn't too big to ensure that
            // it lives solely within the first page as we currently only
            // have the guarantee that the first page of memory is unmapped,
            // no more.
            const_assert!(size_of::<VMFuncRef>() <= 512);
            return None;
        }

        let mut fault = None;
        for instance in self.stored.instances.iter() {
            if let Some(f) = instance.handle.wasm_fault(faulting_addr) {
                assert!(fault.is_none());
                fault = Some(f);
            }
        }
        if fault.is_some() {
            return fault;
        }

        tracing::error!(
            "\
k23 caught a segfault for a wasm program because the faulting instruction
is allowed to segfault due to how linear memories are implemented. The address
that was accessed, however, is not known to any linear memory in use within this
Store. This may be indicative of a critical bug in k23's code generation
because all addresses which are known to be reachable from wasm won't reach this
message.

    pc:      0x{pc}
    address: 0x{faulting_addr}

This is a possible security issue because WebAssembly has accessed something it
shouldn't have been able to. Other accesses may have succeeded and this one just
happened to be caught.
"
        );
        arch::abort("");
    }
}
