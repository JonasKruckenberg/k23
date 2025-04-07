// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::wasm::module_registry::ModuleRegistry;
use crate::wasm::values::Val;
use crate::wasm::vm::{InstanceAllocator, InstanceHandle, VMContext, VMStoreContext, VMVal};
use crate::wasm::{Engine, Module};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Formatter;
use core::marker::{PhantomData, PhantomPinned};
use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;
use core::{fmt, mem};
use static_assertions::assert_impl_all;

pub struct Store<T>(ManuallyDrop<Box<StoreInner<T>>>);

#[repr(C)]
struct StoreInner<T> {
    // this needs to be the first field
    inner: StoreOpaque,
    data: ManuallyDrop<T>,
}

pub struct StoreOpaque {
    engine: Engine,
    alloc: Box<dyn InstanceAllocator + Send + Sync>,
    vm_store_context: VMStoreContext,
    modules: ModuleRegistry,

    /// TODO explain
    default_caller: InstanceHandle,

    /// Indexed data within this `Store`, used to store information about
    /// globals, functions, memories, etc.
    ///
    /// Note that this is `ManuallyDrop` because it needs to be dropped before
    /// `rooted_host_funcs` below. This structure contains pointers which are
    /// otherwise kept alive by the `Arc` references in `rooted_host_funcs`.
    data: ManuallyDrop<StoreData>,

    /// Used to optimized wasm->host calls when the host function is defined with
    /// `Func::new` to avoid allocating a new vector each time a function is
    /// called.
    hostcall_val_storage: Vec<Val>,
    /// Same as `hostcall_val_storage`, but for the direction of the host
    /// calling wasm.
    wasm_vmval_storage: Vec<VMVal>,

    _m: PhantomPinned,
}
assert_impl_all!(StoreOpaque: Send, Sync);

#[derive(Debug, Default)]
struct StoreData {
    funcs: Vec<crate::wasm::func::FuncData>,
    tables: Vec<crate::wasm::vm::ExportedTable>,
    globals: Vec<crate::wasm::vm::ExportedGlobal>,
    instances: Vec<crate::wasm::instance::InstanceData>,
    memories: Vec<crate::wasm::vm::ExportedMemory>,
    tags: Vec<crate::wasm::vm::ExportedTag>,
}

impl<T> Store<T> {
    pub fn new(engine: &Engine, alloc: Box<dyn InstanceAllocator + Send + Sync>, data: T) -> Self {
        let mut inner = Box::new(StoreInner {
            inner: StoreOpaque {
                engine: engine.clone(),
                alloc,
                vm_store_context: VMStoreContext::default(),
                modules: ModuleRegistry::default(),
                default_caller: InstanceHandle::null(),
                data: ManuallyDrop::new(StoreData::default()),
                hostcall_val_storage: vec![],
                wasm_vmval_storage: vec![],
                _m: PhantomPinned,
            },
            data: ManuallyDrop::new(data),
        });

        inner.inner.default_caller = {
            let mut instance = inner
                .inner
                .alloc
                .allocate_module(Module::new_stub(engine.clone()))
                .expect("failed to allocate default callee");

            unsafe {
                instance
                    .instance_mut()
                    .set_store(Some(NonNull::from(&mut inner.inner)));
            }

            instance
        };

        Self(ManuallyDrop::new(inner))
    }
}

impl<T> Deref for Store<T> {
    type Target = StoreOpaque;
    fn deref(&self) -> &Self::Target {
        &self.0.inner
    }
}

impl<T> DerefMut for Store<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0.inner
    }
}

// === impl StoreOpaque ===

impl fmt::Debug for StoreOpaque {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreOpaque").finish_non_exhaustive()
    }
}

impl StoreOpaque {
    #[inline]
    pub(super) fn engine(&self) -> &Engine {
        &self.engine
    }
    #[inline]
    pub(super) fn alloc(&mut self) -> &mut (dyn InstanceAllocator + Send + Sync) {
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

impl<T> Drop for Store<T> {
    fn drop(&mut self) {
        // self.inner.flush_fiber_stack();

        // for documentation on this `unsafe`, see `into_data`.
        unsafe {
            ManuallyDrop::drop(&mut self.0.data);
        }
    }
}

impl Drop for StoreOpaque {
    fn drop(&mut self) {
        unsafe {
            for data in self.data.instances.iter_mut() {
                self.alloc.deallocate_module(&mut data.handle);
            }

            ManuallyDrop::drop(&mut self.data);
            // ManuallyDrop::drop(&mut self.rooted_host_funcs);
        }
    }
}

// === impl Stored ===

macro_rules! stored_impls {
    ($bind:ident $(($ty:path, $add:ident, $has:ident, $get:ident, $get_mut:ident, $field:expr))*) => {
        $(
            impl StoreOpaque {
                pub fn $add(&mut self, val: $ty) -> Stored<$ty> {
                    let $bind = self;
                    let index = $field.len();
                    $field.push(val);
                    Stored::new(index)
                }

                // #[expect(missing_docs, reason = "inside macro")]
                pub(super) fn $has(&self, index: Stored<$ty>) -> bool {
                    let $bind = self;
                    $field.get(index.index).is_some()
                }

                // #[expect(missing_docs, reason = "inside macro")]
                pub(super) fn $get(&self, index: Stored<$ty>) -> Option<&$ty> {
                    let $bind = self;
                    $field.get(index.index)
                }

                // #[expect(missing_docs, reason = "inside macro")]
                pub(super) fn $get_mut(&mut self, index: Stored<$ty>) -> Option<&mut $ty> {
                    let $bind = self;
                    $field.get_mut(index.index)
                }
            }

            impl ::core::ops::Index<Stored<$ty>> for StoreOpaque {
                type Output = $ty;

                fn index(&self, index: Stored<$ty>) -> &Self::Output {
                    self.$get(index).unwrap()
                }
            }

            impl ::core::ops::IndexMut<Stored<$ty>> for StoreOpaque {
                fn index_mut(&mut self, index: Stored<$ty>) -> &mut Self::Output {
                    self.$get_mut(index).unwrap()
                }
            }
        )*
    };
}

stored_impls! {
    s
    (crate::wasm::instance::InstanceData, add_instance, has_instance, get_instance, get_instance_mut, s.data.instances)
    (crate::wasm::func::FuncData, add_function, has_function, get_function, get_function_mut, s.data.funcs)
    (crate::wasm::vm::ExportedTable, add_table, has_table, get_table, get_table_mut, s.data.tables)
    (crate::wasm::vm::ExportedMemory, add_memory, has_memory, get_memory, get_memory_mut, s.data.memories)
    (crate::wasm::vm::ExportedGlobal, add_global, has_global, get_global, get_global_mut, s.data.globals)
}

pub struct Stored<T> {
    index: usize,
    _m: PhantomData<T>,
}

impl<T> Stored<T> {
    pub fn new(index: usize) -> Self {
        Self {
            index,
            _m: PhantomData,
        }
    }
}

impl<T> Clone for Stored<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Stored<T> {}

impl<T> fmt::Debug for Stored<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Stored").field(&self.index).finish()
    }
}
