//! Support for safe & efficient stack switching in k23.
//!
//! This crate provides the [`Fiber`] which implements stackful [`coroutines`]. These are used as
//! the basis for Wasm guest multitasking in k23 which requires a stack (as opposed to the k23 kernel
//! multitasking which uses stack*less* Rust futures).
//!
//! This crate is heavily based off of [`corosensei`] by Amanieu d'Antras which a few k23 specific
//! changes (notably the addition of associated fiber-local data, see [`Fiber::fiber_local`]).
//!
//! [`coroutines`]: https://en.wikipedia.org/wiki/Coroutine
//! [`corosensei`]: https://github.com/Amanieu/corosensei

#![cfg_attr(all(not(test), target_os = "none"), no_std)]
#![feature(naked_functions)]
#![feature(asm_unwind)]

mod arch;
pub mod stack;
mod utils;

use crate::stack::{FiberStack, StackPointer};
use crate::utils::EncodedValue;
use core::cell::Cell;
use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::mem::{MaybeUninit, offset_of};
use core::ptr;

/// Value returned from resuming a fiber.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FiberResult<Yield, Return> {
    /// Value returned by a fiber suspending itself with a `Yielder`.
    Yield(Yield),

    /// Value returned by a fiber returning from its main function.
    Return(Return),
}

impl<Yield, Return> FiberResult<Yield, Return> {
    /// Returns the `Yield` value as an `Option<Yield>`.
    pub fn into_yield(self) -> Option<Yield> {
        match self {
            FiberResult::Yield(val) => Some(val),
            FiberResult::Return(_) => None,
        }
    }

    /// Returns the `Return` value as an `Option<Return>`.
    pub fn into_return(self) -> Option<Return> {
        match self {
            FiberResult::Yield(_) => None,
            FiberResult::Return(val) => Some(val),
        }
    }
}

pub struct Fiber<Input, Yield, Return, L, S: FiberStack> {
    /// Stack that the fiber is executing on.
    stack: S,
    /// Current stack pointer at which the fiber state is held. This is
    /// None when the fiber has completed execution.
    stack_ptr: Option<StackPointer>,
    /// Initial stack pointer value. This is used to detect whether a fiber
    /// has ever been resumed since it was created.
    ///
    /// This works because it is impossible for a fiber to revert back to its
    /// initial stack pointer: suspending a fiber requires pushing several
    /// values to the stack.
    initial_stack_ptr: StackPointer,
    // /// Function to call to drop the initial state of a fiber if it has
    // /// never been resumed.
    // drop_fn: unsafe fn(ptr: *mut u8),
    fiber_local: *const L,
    /// We want to be covariant over Yield and Return, and contravariant
    /// over Input.
    ///
    /// Effectively this means that we can pass a
    ///   Fiber<&'a (), &'static (), &'static ()>
    /// to a function that expects a
    ///   Fiber<&'static (), &'c (), &'d ()>
    _m1: PhantomData<fn(Input) -> FiberResult<Yield, Return>>,
    /// Fiber must be !Send.
    /// ```compile_fail
    /// fn send<T: Send>() {}
    /// send::<fiber::Fiber<(), ()>>();
    /// ```
    _m2: PhantomData<*mut ()>,
}

impl<Input, Yield, Return, L: Default, S: FiberStack> Fiber<Input, Yield, Return, L, S> {
    pub fn with_stack<F>(stack: S, f: F) -> Self
    where
        F: FnOnce(Input, &Suspend<Input, Yield>, &L) -> Return,
        F: 'static,
        Input: 'static,
        Yield: 'static,
        Return: 'static,
    {
        Self::with_stack_and_local(stack, L::default(), f)
    }
}

impl<Input, Yield, Return, L, S: FiberStack> Fiber<Input, Yield, Return, L, S> {
    /// Crates a new fiber from the provided [`FiberStack`] and fiber-local value.
    ///
    /// The fiber local will be stored at the top of the stack, and will be accessible for the lifetime
    /// of the fiber.
    pub fn with_stack_and_local<F>(stack: S, fiber_local: L, func: F) -> Self
    where
        F: FnOnce(Input, &Suspend<Input, Yield>, &L) -> Return,
        F: 'static,
        Input: 'static,
        Yield: 'static,
        Return: 'static,
    {
        #[repr(C)]
        struct InitialObject<L, F> {
            fiber_local: L,
            func: MaybeUninit<F>,
        }

        unsafe extern "C-unwind" fn fiber_func<Input, Yield, Return, L, F>(
            input: EncodedValue,
            parent_link: &mut StackPointer,
            obj: *mut InitialObject<L, F>,
        ) -> !
        where
            F: FnOnce(Input, &Suspend<Input, Yield>, &L) -> Return,
        {
            // Safety: TODO
            unsafe {
                // The suspend is a #[repr(transparent)] wrapper around the
                // parent link on the stack.
                let suspend = &*(ptr::from_mut(parent_link).cast::<Suspend<Input, Yield>>());

                // Read the initial object from the stack.
                debug_assert_eq!(obj as usize % align_of::<F>(), 0);
                let obj = obj.as_ref().unwrap();

                let input: Input = utils::decode_val(input);

                // Run the body of the generator
                let result = obj.func.assume_init_read()(input, suspend, &obj.fiber_local);

                // Return any caught panics to the parent context.
                let mut result = ManuallyDrop::new(result);
                arch::switch_and_reset(utils::encode_val(&mut result), suspend.stack_ptr.as_ptr());
            }
        }

        // // Drop function to free the initial state of the fiber.
        // unsafe fn drop_fn<T, L>(ptr: *mut u8) {
        //     // Safety: TODO
        //     unsafe {
        //         // drop the initial object
        //         ptr::drop_in_place(ptr.cast::<T>());
        //         // drop the fiber-local state
        //         ptr::drop_in_place(ptr.cast::<L>());
        //     }
        // }

        // Safety: TODO
        unsafe {
            // Set up the stack so that the fiber starts executing
            // fiber_func. Write the given function object to the stack so
            // its address is passed to fiber_func on the first resume.
            let (stack_ptr, init_obj) = arch::init_stack(
                &stack,
                fiber_func::<Input, Yield, Return, L, F>,
                InitialObject {
                    fiber_local,
                    func: MaybeUninit::new(func),
                },
            );

            let fiber_local = {
                let addr = init_obj.get() + offset_of!(InitialObject<L, F>, fiber_local);
                addr as *const L
            };

            Self {
                stack,
                stack_ptr: Some(stack_ptr),
                initial_stack_ptr: stack_ptr,
                // drop_fn: drop_fn::<F, L>,
                fiber_local,
                _m1: PhantomData,
                _m2: PhantomData,
            }
        }
    }

    /// Resume a suspended fiber, the `Input` value will be passed to the fiber and returned by [`Suspend::suspend`].
    ///
    /// # Panics
    ///
    /// Panics if the fiber is already completed.
    pub fn resume(&mut self, input: Input) -> FiberResult<Yield, Return> {
        let mut input = ManuallyDrop::new(input);

        let stack_ptr = self
            .stack_ptr
            .take()
            .expect("attempt to resume a completed fiber");

        // Safety: TODO
        unsafe {
            let (result, stack_ptr) =
                arch::switch_and_link(utils::encode_val(&mut input), stack_ptr, self.stack.top());

            self.stack_ptr = stack_ptr;

            // Decode the returned value depending on whether the fiber
            // terminated.
            if stack_ptr.is_some() {
                FiberResult::Yield(utils::decode_val(result))
            } else {
                FiberResult::Return(utils::decode_val(result))
            }
        }
    }

    /// Returns whether this fiber has been resumed at least once.
    pub fn started(&self) -> bool {
        self.stack_ptr != Some(self.initial_stack_ptr)
    }

    /// Returns whether this fiber has finished executing.
    ///
    /// A fiber that has returned from its initial function can no longer
    /// be resumed.
    pub fn done(&self) -> bool {
        self.stack_ptr.is_none()
    }

    /// Forcibly marks the fiber as having completed, even if it is
    /// currently suspended in the middle of a function.
    ///
    /// # Safety
    ///
    /// This is equivalent to a `longjmp` all the way back to the initial
    /// function of the fiber, so the same rules apply.
    ///
    /// This can only be done safely if there are no objects currently on the
    /// fiber's stack that need to execute `Drop` code.
    pub unsafe fn force_reset(&mut self) {
        self.stack_ptr = None;
    }

    /// Return a reference to the fiber-local state associated with this fiber.
    #[expect(clippy::missing_panics_doc, reason = "not a user-facing panic")]
    pub fn fiber_local(&self) -> &L {
        // Safety: the fiber-local value is always initialized by construction
        unsafe {
            self.fiber_local
                .as_ref()
                .expect("fiber-local pointer was null, this is a bug!")
        }
    }

    // /// Unwinds the fiber stack, dropping any live objects that are
    // /// currently on the stack. This is automatically called when the fiber
    // /// is dropped.
    // ///
    // /// If the fiber has already completed then this function is a no-op.
    // ///
    // /// If the fiber is currently suspended on a `Yielder::suspend` call
    // /// then unwinding it requires the `unwind` feature to be enabled and
    // /// for the crate to be compiled with `-C panic=unwind`.
    // ///
    // /// # Panics
    // ///
    // /// This function panics if the fiber could not be fully unwound. This
    // /// can happen for one of two reasons:
    // /// - The `ForcedUnwind` panic that is used internally was caught and not
    // ///   rethrown.
    // /// - This crate was compiled without the `unwind` feature and the
    // ///   fiber is currently suspended in the yielder (`started && !done`).
    // pub unsafe fn force_unwind(&mut self) {
    //     // If the fiber has already terminated then there is nothing to do.
    //     if let Some(stack_ptr) = self.stack_ptr.take() {
    //         self.force_unwind_slow(stack_ptr);
    //     }
    // }
    //
    // /// Slow path of `force_unwind` when the fiber is known to not have
    // /// terminated yet.
    // #[cold]
    // fn force_unwind_slow(&mut self, stack_ptr: StackPointer) {
    //     // Safety: TODO
    //     unsafe {
    //         // If the fiber has not started yet then we just need to drop the
    //         // initial object.
    //         if !self.started() {
    //             arch::drop_initial_obj(self.stack.top(), stack_ptr, self.drop_fn);
    //
    //             self.stack_ptr = None;
    //             return;
    //         }
    //
    //         let res = crate::panic::catch_unwind(AssertUnwindSafe(|| {
    //             arch::switch_and_throw(stack_ptr, self.stack.top())
    //         }));
    //         // we expect the forced unwinding to bubble up to this catch_unwind
    //         assert!(res.is_err());
    //     }
    // }
}

impl<Input, Yield, Return, L, S: FiberStack> Drop for Fiber<Input, Yield, Return, L, S> {
    fn drop(&mut self) {
        assert!(self.done());
    }
}

#[repr(transparent)]
pub struct Suspend<Input, Yield> {
    // Internally the Yielder is just the parent link on the stack which is
    // updated every time resume() is called.
    stack_ptr: Cell<StackPointer>,
    marker: PhantomData<fn(Yield) -> Input>,
}

impl<Input, Yield> Suspend<Input, Yield> {
    /// Suspends the execution of the calling fiber.
    ///
    /// This will yield back control to the original caller of [`Fiber::resume`] transferring the provided
    /// `Yield` argument to it as the return of `resume`.
    pub fn suspend(&self, val: Yield) -> Input {
        // Safety: TODO
        unsafe {
            let mut val = ManuallyDrop::new(val);
            let result = arch::switch_yield(utils::encode_val(&mut val), self.stack_ptr.as_ptr());

            utils::decode_val(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Fiber;
    use crate::stack::DefaultFiberStack;
    use std::cell::Cell;

    #[test]
    fn it_works() {
        let stack = DefaultFiberStack::default();

        let mut fiber = Fiber::with_stack(stack, |input, suspend, _local: &()| {
            assert_eq!(input, 100);

            for i in 0..5 {
                let input = suspend.suspend(i);
                assert_eq!(input, 100 + i + 1);
            }
        });

        // assert that we can resume the fiber 5 times and that we are correctly passing the inputs/yields
        assert_eq!(fiber.resume(100).into_yield().unwrap(), 0);
        assert_eq!(fiber.resume(101).into_yield().unwrap(), 1);
        assert_eq!(fiber.resume(102).into_yield().unwrap(), 2);
        assert_eq!(fiber.resume(103).into_yield().unwrap(), 3);
        assert_eq!(fiber.resume(104).into_yield().unwrap(), 4);

        assert!(fiber.resume(105).into_return().is_some())
    }

    #[test]
    fn fiber_local() {
        let stack = DefaultFiberStack::default();

        let mut fiber = Fiber::with_stack(stack, |input, suspend, local: &Cell<i32>| {
            let prev = local.replace(input);
            let input = suspend.suspend(prev);

            let prev = local.replace(input);
            let input = suspend.suspend(prev);

            local.replace(input);
        });

        assert_eq!(fiber.fiber_local().get(), 0);

        assert_eq!(fiber.resume(1).into_yield().unwrap(), 0);
        assert_eq!(fiber.fiber_local().get(), 1);

        assert_eq!(fiber.resume(2).into_yield().unwrap(), 1);
        assert_eq!(fiber.fiber_local().get(), 2);

        assert!(fiber.resume(42).into_return().is_some())
    }
}
