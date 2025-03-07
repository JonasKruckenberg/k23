// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::fiber::{Fiber, FiberStack, Suspend};
use crate::vm::frame_alloc::FrameAllocator;
use crate::wasm::instance_allocator::PlaceholderAllocatorDontUse;
use crate::wasm::runtime::{VMContext, VMOpaqueContext, VMVal};
use crate::wasm::trap_handler::{AsyncActivation, PreviousAsyncActivation};
use crate::wasm::{Engine, Error, InstanceAllocator, runtime};
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::range::Range;
use core::task::{Context, Poll};
use core::{fmt, mem, ptr};
use core::hash::{Hash, Hasher};
use hashbrown::HashMap;
use static_assertions::assert_impl_all;

/// A store owns WebAssembly instances and their associated data (tables, memories, globals and functions).
#[derive(Debug)]
pub struct Store {
    pub(crate) engine: Engine,
    instances: Vec<runtime::Instance>,
    funcs: Vec<super::func::FuncInner>,
    exported_tables: Vec<runtime::ExportedTable>,
    exported_memories: Vec<runtime::ExportedMemory>,
    exported_globals: Vec<runtime::ExportedGlobal>,
    wasm_vmval_storage: Vec<VMVal>,

    vmctx2instance: Vmctx2Instance,

    pub(super) alloc: PlaceholderAllocatorDontUse,
    async_state: AsyncState,
}
assert_impl_all!(Store: Send, Sync);

#[derive(Debug)]
struct Vmctx2Instance(HashMap<*mut VMOpaqueContext, Stored<runtime::Instance>>);

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Send for Vmctx2Instance {}
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Sync for Vmctx2Instance {}

#[derive(Debug)]
struct AsyncState {
    current_suspend: UnsafeCell<*mut Suspend<super::Result<()>, (), super::Result<()>>>,
    current_poll_cx: UnsafeCell<PollContext>,
    /// The last fiber stack that was in use by this store.
    last_fiber_stack: Option<FiberStack>,
}

impl Default for AsyncState {
    fn default() -> AsyncState {
        AsyncState {
            current_suspend: UnsafeCell::new(ptr::null_mut()),
            current_poll_cx: UnsafeCell::new(PollContext::default()),
            last_fiber_stack: None,
        }
    }
}

#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Send for AsyncState {}
#[expect(clippy::undocumented_unsafe_blocks, reason = "")]
unsafe impl Sync for AsyncState {}

#[derive(Debug, Copy, Clone)]
struct PollContext {
    future_context: *mut Context<'static>,
    guard_range_start: *mut u8,
    guard_range_end: *mut u8,
}

impl Default for PollContext {
    fn default() -> PollContext {
        PollContext {
            future_context: ptr::null_mut(),
            guard_range_start: ptr::null_mut(),
            guard_range_end: ptr::null_mut(),
        }
    }
}

impl Store {
    /// Constructs a new store with the given engine.
    pub fn new(engine: &Engine, frame_alloc: &'static FrameAllocator) -> Self {
        Self {
            engine: engine.clone(),
            instances: Vec::new(),
            funcs: Vec::new(),
            exported_tables: Vec::new(),
            exported_memories: Vec::new(),
            exported_globals: Vec::new(),
            wasm_vmval_storage: Vec::new(),

            vmctx2instance: Vmctx2Instance(HashMap::new()),

            alloc: PlaceholderAllocatorDontUse::new(engine, frame_alloc),
            async_state: AsyncState::default(),
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
        func: super::func::FuncInner,
    ) -> Stored<super::func::FuncInner> {
        let index = self.funcs.len();
        self.funcs.push(func);
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

    pub(super) async fn on_fiber<R>(
        &mut self,
        func: impl FnOnce(&mut Self) -> R + Send,
    ) -> super::Result<R> {
        let mut slot = None;
        let mut future = {
            let current_poll_cx = self.async_state.current_poll_cx.get();
            let current_suspend = self.async_state.current_suspend.get();
            let stack = self.alloc.allocate_fiber_stack()?;

            let slot = &mut slot;
            let this = &mut *self;
            let fiber = Fiber::new(stack, move |keep_going, suspend| {
                // First check and see if we were interrupted/dropped, and only
                // continue if we haven't been.
                keep_going?;

                // Configure our store's suspension context for the rest of the
                // execution of this fiber.
                // Safety: a raw pointer is stored here
                // which is only valid for the duration of this closure.
                // Consequently, we at least replace it with the previous value when
                // we're done. This reset is also required for correctness because
                // otherwise our value will overwrite another active fiber's value.
                // There should be a test that segfaults in `async_functions.rs` if
                // this `Replace` is removed.
                unsafe {
                    let _reset = Reset(current_suspend, *current_suspend);
                    *current_suspend = suspend;

                    *slot = Some(func(this));
                    Ok(())
                }
            });

            // Once we have the fiber representing our synchronous computation, we
            // wrap that in a custom future implementation which does the
            // translation from the future protocol to our fiber API.
            FiberFuture {
                fiber: Some(fiber),
                current_poll_cx,
                // alloc: &this.alloc,
                state: Some(AsyncActivation::new()),
            }
        };
        (&mut future).await?;
        let stack = future.fiber.take().map(|f| f.into_stack());
        drop(future);
        if let Some(stack) = stack {
            // Safety: we previously allocated the stack above, so all is fine
            unsafe {
                self.alloc.deallocate_fiber_stack(stack);
            }
        }

        return Ok(slot.unwrap());

        struct FiberFuture<'a> {
            fiber: Option<Fiber<'a, super::Result<()>, (), super::Result<()>>>,
            current_poll_cx: *mut PollContext,
            // alloc: &'a dyn InstanceAllocator,
            // engine: Engine,
            // See comments in `FiberFuture::resume` for this
            state: Option<AsyncActivation>,
        }

        // Safety: This is surely the most dangerous `unsafe impl Send` in the entire
        // crate. There are two members in `FiberFuture` which cause it to not
        // be `Send`. One is `current_poll_cx` and is entirely uninteresting.
        // This is just used to manage `Context` pointers across `await` points
        // in the future, and requires raw pointers to get it to happen easily.
        // Nothing too weird about the `Send`-ness, values aren't actually
        // crossing threads.
        //
        // The really interesting piece is `fiber`. Now the "fiber" here is
        // actual honest-to-god Rust code which we're moving around. What we're
        // doing is the equivalent of moving our thread's stack to another OS
        // thread. Turns out we, in general, have no idea what's on the stack
        // and would generally have no way to verify that this is actually safe
        // to do!
        //
        // Thankfully, though, Wasmtime has the power. Without being glib it's
        // actually worth examining what's on the stack. It's unfortunately not
        // super-local to this function itself. Our closure to `Fiber::new` runs
        // `func`, which is given to us from the outside. Thankfully, though, we
        // have tight control over this. Usage of `on_fiber` is typically done
        // *just* before entering WebAssembly itself, so we'll have a few stack
        // frames of Rust code (all in Wasmtime itself) before we enter wasm.
        //
        // Once we've entered wasm, well then we have a whole bunch of wasm
        // frames on the stack. We've got this nifty thing called Cranelift,
        // though, which allows us to also have complete control over everything
        // on the stack!
        //
        // Finally, when wasm switches back to the fiber's starting pointer
        // (this future we're returning) then it means wasm has reentered Rust.
        // Suspension can only happen via the `block_on` function of an
        // `AsyncCx`. This, conveniently, also happens entirely in Wasmtime
        // controlled code!
        //
        // There's an extremely important point that should be called out here.
        // User-provided futures **are not on the stack** during suspension
        // points. This is extremely crucial because we in general cannot reason
        // about Send/Sync for stack-local variables since rustc doesn't analyze
        // them at all. With our construction, though, we are guaranteed that
        // Wasmtime owns all stack frames between the stack of a fiber and when
        // the fiber suspends (and it could move across threads). At this time
        // the only user-provided piece of data on the stack is the future
        // itself given to us. Lo-and-behold as you might notice the future is
        // required to be `Send`!
        //
        // What this all boils down to is that we, as the authors of Wasmtime,
        // need to be extremely careful that on the async fiber stack we only
        // store Send things. For example we can't start using `Rc` willy nilly
        // by accident and leave a copy in TLS somewhere. (similarly we have to
        // be ready for TLS to change while we're executing wasm code between
        // suspension points).
        //
        // While somewhat onerous it shouldn't be too too hard (the TLS bit is
        // the hardest bit so far). This does mean, though, that no user should
        // ever have to worry about the `Send`-ness of Wasmtime. If rustc says
        // it's ok, then it's ok.
        //
        // With all that in mind we unsafely assert here that wasmtime is
        // correct. We declare the fiber as only containing Send data on its
        // stack, despite not knowing for sure at compile time that this is
        // correct. That's what `unsafe` in Rust is all about, though, right?
        unsafe impl Send for FiberFuture<'_> {}

        impl FiberFuture<'_> {
            fn fiber(&self) -> &Fiber<'_, super::Result<()>, (), super::Result<()>> {
                self.fiber.as_ref().unwrap()
            }

            /// This is a helper function to call `resume` on the underlying
            /// fiber while correctly managing Wasmtime's thread-local data.
            ///
            /// Wasmtime's implementation of traps leverages thread-local data
            /// to get access to metadata during a signal. This thread-local
            /// data is a linked list of "activations" where the nodes of the
            /// linked list are stored on the stack. It would be invalid as a
            /// result to suspend a computation with the head of the linked list
            /// on this stack then move the stack to another thread and resume
            /// it. That means that a different thread would point to our stack
            /// and our thread doesn't point to our stack at all!
            ///
            /// Basically management of TLS is required here one way or another.
            /// The strategy currently settled on is to manage the list of
            /// activations created by this fiber as a unit. When a fiber
            /// resumes the linked list is prepended to the current thread's
            /// list. When the fiber is suspended then the fiber's list of
            /// activations are all removed en-masse and saved within the fiber.
            fn resume(&mut self, val: super::Result<()>) -> Result<super::Result<()>, ()> {
                // Safety: TODO
                unsafe {
                    let prev = self.state.take().unwrap().push();
                    let restore = Restore {
                        fiber: self,
                        state: Some(prev),
                    };
                    return restore.fiber.fiber().resume(val);
                }

                struct Restore<'a, 'b> {
                    fiber: &'a mut FiberFuture<'b>,
                    state: Option<PreviousAsyncActivation>,
                }

                impl Drop for Restore<'_, '_> {
                    fn drop(&mut self) {
                        // Safety: TODO
                        unsafe {
                            self.fiber.state = Some(self.state.take().unwrap().restore());
                        }
                    }
                }
            }
        }

        impl Future for FiberFuture<'_> {
            type Output = super::Result<()>;

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
                let guard = self
                    .fiber()
                    .stack()
                    .guard_range()
                    .unwrap_or(Range::from(ptr::null_mut()..ptr::null_mut()));
                // Safety: TODO
                unsafe {
                    let _reset = Reset(self.current_poll_cx, *self.current_poll_cx);
                    *self.current_poll_cx = PollContext {
                        future_context: mem::transmute::<&mut Context<'_>, *mut Context<'static>>(
                            cx,
                        ),
                        guard_range_start: guard.start,
                        guard_range_end: guard.end,
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
                            AsyncActivation::assert_current_state_not_in_range(
                                self.fiber().stack().range(),
                            );

                            Poll::Pending
                        }
                    }
                }
            }
        }

        // Dropping futures is pretty special in that it means the future has
        // been requested to be cancelled. Here we run the risk of dropping an
        // in-progress fiber, and if we were to do nothing then the fiber would
        // leak all its owned stack resources.
        //
        // To handle this we implement `Drop` here and, if the fiber isn't done,
        // resume execution of the fiber saying "hey please stop you're
        // interrupted". Our `Trap` created here (which has the stack trace
        // of whomever dropped us) will then get propagated in whatever called
        // `block_on`, and the idea is that the trap propagates all the way back
        // up to the original fiber start, finishing execution.
        //
        // We don't actually care about the fiber's return value here (no one's
        // around to look at it), we just assert the fiber finished to
        // completion.
        impl Drop for FiberFuture<'_> {
            fn drop(&mut self) {
                if self.fiber.is_none() {
                    return;
                }

                if !self.fiber().done() {
                    let result = self.resume(Err(Error::FutureDropped));
                    // This resumption with an error should always complete the
                    // fiber. While it's technically possible for host code to catch
                    // the trap and re-resume, we'd ideally like to signal that to
                    // callers that they shouldn't be doing that.
                    debug_assert!(result.is_ok());
                }

                self.state.take().unwrap().assert_null();
            }
        }

        struct Reset<T: Copy>(*mut T, T);

        impl<T: Copy> Drop for Reset<T> {
            fn drop(&mut self) {
                // Safety: TODO
                unsafe {
                    *self.0 = self.1;
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
    (super::func::FuncInner, has_function, get_function, get_function_mut, s.funcs)
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
impl<T> PartialEq for Stored<T> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}
impl<T> Eq for Stored<T> {}