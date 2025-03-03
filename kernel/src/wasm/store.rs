use crate::vm::frame_alloc::FrameAllocator;
use crate::wasm::instance_allocator::PlaceholderAllocatorDontUse;
use crate::wasm::runtime::{VMContext, VMOpaqueContext, VMVal};
use crate::wasm::{Engine, runtime};
use alloc::vec::Vec;
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::{fmt, mem, ptr};
use hashbrown::HashMap;
use static_assertions::assert_impl_all;

/// A store owns WebAssembly instances and their associated data (tables, memories, globals and functions).
#[derive(Debug)]
pub struct Store {
    pub(crate) engine: Engine,
    instances: Vec<runtime::Instance>,
    exported_funcs: Vec<runtime::ExportedFunction>,
    exported_tables: Vec<runtime::ExportedTable>,
    exported_memories: Vec<runtime::ExportedMemory>,
    exported_globals: Vec<runtime::ExportedGlobal>,
    wasm_vmval_storage: Vec<VMVal>,

    vmctx2instance: Vmctx2Instance,

    pub(super) alloc: PlaceholderAllocatorDontUse,
}
assert_impl_all!(Store: Send, Sync);

#[derive(Debug)]
struct Vmctx2Instance(HashMap<*mut VMOpaqueContext, Stored<runtime::Instance>>);

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Send for Vmctx2Instance {}
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Sync for Vmctx2Instance {}

impl Store {
    /// Constructs a new store with the given engine.
    pub fn new(engine: &Engine, frame_alloc: &'static FrameAllocator) -> Self {
        Self {
            engine: engine.clone(),
            instances: Vec::new(),
            exported_funcs: Vec::new(),
            exported_tables: Vec::new(),
            exported_memories: Vec::new(),
            exported_globals: Vec::new(),
            wasm_vmval_storage: Vec::new(),

            vmctx2instance: Vmctx2Instance(HashMap::new()),

            alloc: PlaceholderAllocatorDontUse::new(engine, frame_alloc),
        }
    }

    /// Takes the `Vec<VMVal>` storage used for passing arguments using the array call convention.
    pub(crate) fn take_wasm_vmval_storage(&mut self) -> Vec<VMVal> {
        mem::take(&mut self.wasm_vmval_storage)
    }

    /// Returns the `Vec<VMVal>` storage allowing it's allocation to be reused for the next array call.
    pub(crate) fn return_wasm_vmval_storage(&mut self, storage: Vec<VMVal>) {
        self.wasm_vmval_storage = storage;
    }

    /// Looks up the instance handle associated with the given `vmctx` pointer.
    pub(crate) fn get_instance_from_vmctx(
        &self,
        vmctx: *mut VMContext,
    ) -> Stored<runtime::Instance> {
        let vmctx = VMOpaqueContext::from_vmcontext(vmctx);
        self.vmctx2instance.0[&vmctx]
    }

    /// Inserts a new instance into the store and returns a handle to it.
    pub(crate) fn push_instance(
        &mut self,
        mut instance: runtime::Instance,
    ) -> Stored<runtime::Instance> {
        let handle = Stored::new(self.instances.len());
        self.vmctx2instance.0.insert(
            VMOpaqueContext::from_vmcontext(instance.vmctx_mut()),
            handle,
        );
        self.instances.push(instance);
        handle
    }

    /// Inserts a new function into the store and returns a handle to it.
    pub(crate) fn push_function(
        &mut self,
        func: runtime::ExportedFunction,
    ) -> Stored<runtime::ExportedFunction> {
        let index = self.exported_funcs.len();
        self.exported_funcs.push(func);
        Stored::new(index)
    }

    /// Inserts a new table into the store and returns a handle to it.
    pub(crate) fn push_table(
        &mut self,
        table: runtime::ExportedTable,
    ) -> Stored<runtime::ExportedTable> {
        let index = self.exported_tables.len();
        self.exported_tables.push(table);
        Stored::new(index)
    }

    /// Inserts a new memory into the store and returns a handle to it.
    pub(crate) fn push_memory(
        &mut self,
        memory: runtime::ExportedMemory,
    ) -> Stored<runtime::ExportedMemory> {
        let index = self.exported_memories.len();
        self.exported_memories.push(memory);
        Stored::new(index)
    }

    /// Inserts a new global into the store and returns a handle to it.
    pub(crate) fn push_global(
        &mut self,
        global: runtime::ExportedGlobal,
    ) -> Stored<runtime::ExportedGlobal> {
        let index = self.exported_globals.len();
        self.exported_globals.push(global);
        Stored::new(index)
    }

    pub(super) async fn on_fiber<F, R>(&mut self, f: F) -> crate::wasm::Result<R>
    where
        F: FnOnce(&mut Store) -> R + Send,
    {
        let stack = self.alloc.allocate_fiber_stack()?;
        let mut slot = None;
        let mut current_poll_cx = PollContext::default();
        let this = &mut *self;

        let fiber = Fiber::new(stack, |keep_going, _suspend| {
            // First check and see if we were interrupted/dropped, and only
            // continue if we haven't been.
            keep_going?;

            // Configure our store's suspension context for the rest of the
            // execution of this fiber. Note that a raw pointer is stored here
            // which is only valid for the duration of this closure.
            // Consequently, we at least replace it with the previous value when
            // we're done. This reset is also required for correctness because
            // otherwise our value will overwrite another active fiber's value.
            // There should be a test that segfaults in `async_functions.rs` if
            // this `Replace` is removed.
            // let _reset = Reset(current_suspend, *current_suspend);
            // *current_suspend = suspend;

            slot = Some(f(this));

            Ok(())
        });

        // Once we have the fiber representing our synchronous computation, we
        // wrap that in a custom future implementation which does the
        // translation from the future protocol to our fiber API.
        let mut future = FiberFuture {
            fiber: Some(fiber),
            current_poll_cx: ptr::from_mut(&mut current_poll_cx),
            // alloc: &mut self.alloc,
            // engine,
            // state: Some(crate::runtime::vm::AsyncWasmCallState::new()),
        };
        (&mut future).await?;
        let stack = future.fiber.take().map(|f| f.into_stack());
        drop(future);
        if let Some(stack) = stack {
            // Safety: we're deallocating the stack in the same store it was allocated in
            unsafe {
                self.alloc.deallocate_fiber_stack(stack);
            }
        }

        return Ok(slot.unwrap());

        struct PollContext {
            future_context: *mut Context<'static>,
            // guard_range_start: *mut u8,
            // guard_range_end: *mut u8,
        }

        impl Default for PollContext {
            fn default() -> PollContext {
                PollContext {
                    future_context: ptr::null_mut(),
                    // guard_range_start: core::ptr::null_mut(),
                    // guard_range_end: core::ptr::null_mut(),
                }
            }
        }

        struct FiberFuture<'a> {
            fiber: Option<Fiber<'a, crate::wasm::Result<()>, (), crate::wasm::Result<()>>>,
            current_poll_cx: *mut PollContext,
        }

        // Safety: TODO
        unsafe impl Send for FiberFuture<'_> {}

        impl FiberFuture<'_> {
            fn fiber(&self) -> &Fiber<'_, crate::wasm::Result<()>, (), crate::wasm::Result<()>> {
                self.fiber.as_ref().unwrap()
            }

            fn resume(
                &mut self,
                val: crate::wasm::Result<()>,
            ) -> Result<crate::wasm::Result<()>, ()> {
                self.fiber().resume(val)
            }
        }

        impl Future for FiberFuture<'_> {
            type Output = crate::wasm::Result<()>;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
                // We need to carry over this `cx` into our fiber's runtime
                // for when it tries to poll sub-futures that are created. Doing
                // this must be done unsafely, however, since `cx` is only alive
                // for this one singular function call. Here we do a `transmute`
                // to extend the lifetime of `Context` so it can be stored in
                // our `Store`, and then we replace the current polling context
                // with this one.
                //
                // Note that the replace is done for weird situations where
                // futures might be switching contexts and there's multiple
                // wasmtime futures in a chain of futures.
                //
                // On exit from this function, though, we reset the polling
                // context back to what it was to signify that `Store` no longer
                // has access to this pointer.
                // let guard = self
                //     .fiber()
                //     .stack()
                //     .guard_range()
                //     .unwrap_or(core::ptr::null_mut()..core::ptr::null_mut());
                // Safety: TODO
                unsafe {
                    // let _reset = Reset(self.current_poll_cx, *self.current_poll_cx);
                    *self.current_poll_cx = PollContext {
                        future_context: mem::transmute::<&mut Context<'_>, *mut Context<'static>>(
                            cx,
                        ),
                        // guard_range_start: guard.start,
                        // guard_range_end: guard.end,
                    };

                    // After that's set up we resume execution of the fiber, which
                    // may also start the fiber for the first time. This either
                    // returns `Ok` saying the fiber finished (yay!) or it
                    // returns `Err` with the payload passed to `suspend`, which
                    // in our case is `()`.
                    match self.resume(Ok(())) {
                        Ok(result) => Poll::Ready(result),

                        // If `Err` is returned that means the fiber polled a
                        // future but it said "Pending", so we propagate that
                        // here.
                        //
                        // An additional safety check is performed when leaving
                        // this function to help bolster the guarantees of
                        // `unsafe impl Send` above. Notably this future may get
                        // re-polled on a different thread. Wasmtime's
                        // thread-local state points to the stack, however,
                        // meaning that it would be incorrect to leave a pointer
                        // in TLS when this function returns. This function
                        // performs a runtime assert to verify that this is the
                        // case, notably that the one TLS pointer Wasmtime uses
                        // is not pointing anywhere within the stack. If it is
                        // then that's a bug indicating that TLS management in
                        // Wasmtime is incorrect.
                        Err(()) => {
                            // if let Some(range) = self.fiber().stack().range() {
                            // crate::runtime::vm::AsyncWasmCallState::assert_current_state_not_in_range(range);
                            // }
                            Poll::Pending
                        }
                    }
                }
            }
        }
    }
}

macro_rules! stored_impls {
    ($bind:ident $(($ty:path, $has:ident, $get:ident, $get_mut:ident, $field:expr))*) => {
        $(
            impl Store {
                // #[expect(missing_docs, reason = "inside macro")]
                pub fn $has(&self, index: Stored<$ty>) -> bool {
                    let $bind = self;
                    $field.get(index.index).is_some()
                }

                // #[expect(missing_docs, reason = "inside macro")]
                pub fn $get(&self, index: Stored<$ty>) -> Option<&$ty> {
                    let $bind = self;
                    $field.get(index.index)
                }

                // #[expect(missing_docs, reason = "inside macro")]
                pub fn $get_mut(&mut self, index: Stored<$ty>) -> Option<&mut $ty> {
                    let $bind = self;
                    $field.get_mut(index.index)
                }
            }

            impl ::core::ops::Index<Stored<$ty>> for Store {
                type Output = $ty;

                fn index(&self, index: Stored<$ty>) -> &Self::Output {
                    self.$get(index).unwrap()
                }
            }

            impl ::core::ops::IndexMut<Stored<$ty>> for Store {
                fn index_mut(&mut self, index: Stored<$ty>) -> &mut Self::Output {
                    self.$get_mut(index).unwrap()
                }
            }
        )*
    };
}

stored_impls! {
    s
    (runtime::Instance, has_instance, get_instance, get_instance_mut, s.instances)
    (runtime::ExportedFunction, has_function, get_function, get_function_mut, s.exported_funcs)
    (runtime::ExportedTable, has_table, get_table, get_table_mut, s.exported_tables)
    (runtime::ExportedMemory, has_memory, get_memory, get_memory_mut, s.exported_memories)
    (runtime::ExportedGlobal, has_global, get_global, get_global_mut, s.exported_globals)
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
