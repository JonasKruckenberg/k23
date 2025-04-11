//! ## Linked stacks
//!
//! Stack linking allows a context switch to be automatically performed when the
//! initial function of a context returns or unwinds. This works by stashing a
//! copy of the parent context stack pointer near the stack base and updating it
//! every time we switch into the child context using `switch_and_link`.
//!
//! For unwinding and backtraces to work as expected (that is, to continue in
//! the parent after unwinding past the initial function of a child context),
//! we need to use special DWARF CFI instructions to tell the unwinder how to
//! find the parent frame.
//!
//! If you're curious a decent introduction to CFI things and unwinding is at
//! <https://www.imperialviolet.org/2017/01/18/cfi.html>.

use crate::arch;
use crate::mem::{AddressSpace, Mmap};
use alloc::string::ToString;
use alloc::sync::Arc;
use core::cell::Cell;
use core::marker::PhantomData;
use core::mem::ManuallyDrop;
use core::num::NonZeroUsize;
use core::ops::DerefMut;
use core::ptr;
use core::range::Range;
use spin::Mutex;

/// Value returned from resuming a coroutine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FiberResult<Yield, Return> {
    /// Value returned by a coroutine suspending itself with a `Yielder`.
    Yield(Yield),

    /// Value returned by a coroutine returning from its main function.
    Return(Return),
}

impl<Yield, Return> FiberResult<Yield, Return> {
    /// Returns the `Yield` value as an `Option<Yield>`.
    pub fn as_yield(self) -> Option<Yield> {
        match self {
            FiberResult::Yield(val) => Some(val),
            FiberResult::Return(_) => None,
        }
    }

    /// Returns the `Return` value as an `Option<Return>`.
    pub fn as_return(self) -> Option<Return> {
        match self {
            FiberResult::Yield(_) => None,
            FiberResult::Return(val) => Some(val),
        }
    }
}

#[derive(Debug)]
pub struct FiberStack(Mmap);

impl FiberStack {
    pub fn new(aspace: Arc<Mutex<AddressSpace>>) -> Self {
        let stack_size = 16 * arch::PAGE_SIZE;
        let mmap = Mmap::new_zeroed(
            aspace.clone(),
            stack_size,
            arch::PAGE_SIZE,
            Some("FiberStack".to_string()),
        )
        .unwrap();
        mmap.commit(aspace.lock().deref_mut(), Range::from(0..stack_size), true)
            .unwrap();
        Self(mmap)
    }

    pub fn top(&self) -> StackPointer {
        StackPointer::new(self.0.range().end.get()).unwrap()
    }
}

pub struct Fiber<Input, Yield, Return> {
    // Stack that the coroutine is executing on.
    stack: FiberStack,
    // Current stack pointer at which the coroutine state is held. This is
    // None when the coroutine has completed execution.
    stack_ptr: Option<StackPointer>,
    // Initial stack pointer value. This is used to detect whether a coroutine
    // has ever been resumed since it was created.
    //
    // This works because it is impossible for a coroutine to revert back to its
    // initial stack pointer: suspending a coroutine requires pushing several
    // values to the stack.
    initial_stack_ptr: StackPointer,
    // Function to call to drop the initial state of a coroutine if it has
    // never been resumed.
    drop_fn: unsafe fn(ptr: *mut u8),
    // We want to be covariant over Yield and Return, and contravariant
    // over Input.
    //
    // Effectively this means that we can pass a
    //   Coroutine<&'a (), &'static (), &'static ()>
    // to a function that expects a
    //   Coroutine<&'static (), &'c (), &'d ()>
    marker: PhantomData<fn(Input) -> FiberResult<Yield, Return>>,
    // Coroutine must be !Send.
    /// ```compile_fail
    /// fn send<T: Send>() {}
    /// send::<corosensei::Coroutine<(), ()>>();
    /// ```
    marker2: PhantomData<*mut ()>,
}

impl<Input, Yield, Return> Fiber<Input, Yield, Return> {
    pub fn new<F>(stack: FiberStack, f: F) -> Self
    where
        F: FnOnce(Input, &Suspend<Input, Yield>) -> Return,
        F: 'static,
        Input: 'static,
        Yield: 'static,
        Return: 'static,
    {
        unsafe extern "C-unwind" fn fiber_func<Input, Yield, Return, F>(
            input: EncodedValue,
            parent_link: &mut StackPointer,
            func: *mut F,
        ) -> !
        where
            F: FnOnce(Input, &Suspend<Input, Yield>) -> Return,
        {
            unsafe {
                // The suspend is a #[repr(transparent)] wrapper around the
                // parent link on the stack.
                let suspend = &*(parent_link as *mut StackPointer as *const Suspend<Input, Yield>);

                // Read the function from the stack.
                debug_assert_eq!(func as usize % align_of::<F>(), 0);
                let f = func.read();

                let input: Input = decode_val(input);

                // Run the body of the generator
                let result = f(input, suspend);

                // Return any caught panics to the parent context.
                let mut result = ManuallyDrop::new(result);
                arch::fiber::switch_and_reset(encode_val(&mut result), suspend.stack_ptr.as_ptr());
            }
        }

        // Drop function to free the initial state of the coroutine.
        unsafe fn drop_fn<T>(ptr: *mut u8) {
            unsafe {
                ptr::drop_in_place(ptr as *mut T);
            }
        }

        unsafe {
            // Set up the stack so that the coroutine starts executing
            // coroutine_func. Write the given function object to the stack so
            // its address is passed to coroutine_func on the first resume.
            let stack_ptr =
                arch::fiber::init_stack(&stack, fiber_func::<Input, Yield, Return, F>, f);

            Self {
                stack,
                stack_ptr: Some(stack_ptr),
                initial_stack_ptr: stack_ptr,
                drop_fn: drop_fn::<F>,
                marker: PhantomData,
                marker2: PhantomData,
            }
        }
    }

    pub fn resume(&mut self, input: Input) -> FiberResult<Yield, Return> {
        let mut input = ManuallyDrop::new(input);

        let stack_ptr = self.stack_ptr.take().unwrap();

        unsafe {
            let (result, stack_ptr) =
                arch::fiber::switch_and_link(encode_val(&mut input), stack_ptr, self.stack.top());

            self.stack_ptr = stack_ptr;

            // Decode the returned value depending on whether the coroutine
            // terminated.
            if stack_ptr.is_some() {
                FiberResult::Yield(decode_val(result))
            } else {
                FiberResult::Return(decode_val(result))
            }
        }
    }

    /// Returns whether this coroutine has been resumed at least once.
    pub fn started(&self) -> bool {
        self.stack_ptr != Some(self.initial_stack_ptr)
    }

    /// Returns whether this coroutine has finished executing.
    ///
    /// A coroutine that has returned from its initial function can no longer
    /// be resumed.
    pub fn done(&self) -> bool {
        self.stack_ptr.is_none()
    }

    /// Forcibly marks the coroutine as having completed, even if it is
    /// currently suspended in the middle of a function.
    ///
    /// # Safety
    ///
    /// This is equivalent to a `longjmp` all the way back to the initial
    /// function of the coroutine, so the same rules apply.
    ///
    /// This can only be done safely if there are no objects currently on the
    /// coroutine's stack that need to execute `Drop` code.
    pub unsafe fn force_reset(&mut self) {
        self.stack_ptr = None;
    }
}

impl<Input, Yield, Return> Drop for Fiber<Input, Yield, Return> {
    fn drop(&mut self) {
        assert!(self.done());
    }
}

pub struct Suspend<Input, Yield> {
    // Internally the Yielder is just the parent link on the stack which is
    // updated every time resume() is called.
    stack_ptr: Cell<StackPointer>,
    marker: PhantomData<fn(Yield) -> Input>,
}

impl<Input, Yield> Suspend<Input, Yield> {
    pub fn suspend(&self, val: Yield) -> Input {
        unsafe {
            let mut val = ManuallyDrop::new(val);
            let result = arch::fiber::switch_yield(encode_val(&mut val), self.stack_ptr.as_ptr());

            decode_val(result)
        }
    }
}

pub type StackPointer = NonZeroUsize;

/// Internal type for a value that has been encoded in a `usize`.
pub type EncodedValue = usize;

/// Encodes the given value in a `usize` either directly or as a pointer to the
/// argument. This function logically takes ownership of the value, so it should
/// not be dropped afterwards.
pub unsafe fn encode_val<T>(val: &mut ManuallyDrop<T>) -> EncodedValue {
    unsafe {
        if size_of::<T>() <= size_of::<EncodedValue>() {
            let mut out = 0;
            ptr::write_unaligned(
                &mut out as *mut EncodedValue as *mut T,
                ManuallyDrop::take(val),
            );
            out
        } else {
            val as *const ManuallyDrop<T> as EncodedValue
        }
    }
}

// Decodes a value produced by `encode_usize` either by converting it directly
// or by treating the `usize` as a pointer and dereferencing it.
pub unsafe fn decode_val<T>(val: EncodedValue) -> T {
    unsafe {
        if size_of::<T>() <= size_of::<EncodedValue>() {
            ptr::read_unaligned(&val as *const EncodedValue as *const T)
        } else {
            ptr::read(val as *const T)
        }
    }
}

/// Helper function to push a value onto a stack.
#[inline]
pub unsafe fn push(sp: &mut usize, val: Option<usize>) {
    unsafe {
        *sp -= size_of::<usize>();
        if let Some(val) = val {
            *(*sp as *mut usize) = val;
        }
    }
}

/// Helper function to allocate an object on the stack with proper alignment.
///
/// This function is written such that the stack pointer alignment can be
/// constant-folded away when the object doesn't need an alignment greater than
/// `STACK_ALIGNMENT`.
#[inline]
pub unsafe fn allocate_obj_on_stack<T>(sp: &mut usize, sp_offset: usize, obj: T) {
    unsafe {
        // Sanity check to avoid stack overflows.
        assert!(size_of::<T>() <= 1024, "type is too big to transfer");

        if align_of::<T>() > arch::STACK_ALIGNMENT {
            *sp -= size_of::<T>();
            *sp &= !(align_of::<T>() - 1);
        } else {
            // We know that sp + sp_offset is aligned to STACK_ALIGNMENT. Calculate
            // how much padding we need to add so that sp_offset + padding +
            // sizeof(T) is aligned to STACK_ALIGNMENT.
            let total_size = sp_offset + size_of::<T>();
            let align_offset = total_size % arch::STACK_ALIGNMENT;
            if align_offset != 0 {
                *sp -= arch::STACK_ALIGNMENT - align_offset;
            }
            *sp -= size_of::<T>();
        }
        (*sp as *mut T).write(obj);

        // The stack is aligned to STACK_ALIGNMENT at this point.
        debug_assert_eq!(*sp % arch::STACK_ALIGNMENT, 0);
    }
}

// fn test() {
//     log::debug!("[main] creating coroutine");
//
//     let stack = with_kernel_aspace(|aspace| FiberStack::new(aspace.clone()));
//
//     let mut coroutine = Fiber::new(stack, |input, suspend| {
//         log::debug!("[coroutine] coroutine started with input {}", input);
//         for i in 0..5 {
//             log::debug!("[coroutine] yielding {}", i);
//             let input = suspend.suspend(i);
//             log::debug!("[coroutine] got {} from parent", input)
//         }
//         log::debug!("[coroutine] exiting coroutine");
//     });
//
//     let mut counter = 100;
//     loop {
//         log::debug!("[main] resuming coroutine with argument {}", counter);
//         match coroutine.resume(counter) {
//             FiberResult::Yield(i) => log::debug!("[main] got {:?} from coroutine", i),
//             FiberResult::Return(()) => break,
//         }
//
//         counter += 1;
//     }
//
//     log::debug!("[main] exiting");
// }
