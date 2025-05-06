cfg_if::cfg_if! {
    if #[cfg(all(unix))] {
        mod valgrind;
        mod unix;
        pub use unix::DefaultFiberStack;
    } else if #[cfg(all(windows))] {
        mod valgrind;
        mod windows;
        pub use windows::DefaultFiberStack;
    }
}

pub(crate) type StackPointer = core::num::NonZeroUsize;

/// Minimum size of a stack, excluding guard pages.
pub const MIN_STACK_SIZE: usize = 4096;

pub use crate::arch::STACK_ALIGNMENT;

pub unsafe trait FiberStack {
    /// Returns the highest address (start address) of the stack.
    /// This must be aligned to [`STACK_ALIGNMENT`]
    fn top(&self) -> StackPointer;

    /// Returns the lowest address (maximum limit) of the stack.
    ///
    /// This must include any guard pages and be aligned to [`STACK_ALIGNMENT`]
    fn bottom(&self) -> StackPointer;

    /// On Windows, certain fields must be updated in the Thread Environment
    /// Block when switching to another stack. This function returns the values
    /// that must be assigned for this stack.
    #[cfg(windows)]
    fn teb_fields(&self) -> StackTebFields;

    /// Updates the stack's copy of TEB fields which may have changed while
    /// executing code on the stack.
    #[cfg(windows)]
    fn update_teb_fields(&mut self, stack_limit: usize, guaranteed_stack_bytes: usize);
}

/// Fields in the Thread Environment Block (TEB) which must be updated when
/// switching to a different stack. These are the same fields that are updated
/// by the `SwitchToFiber` function in the Windows API.
#[cfg(windows)]
#[derive(Clone, Copy, Debug)]
#[allow(non_snake_case)]
#[allow(missing_docs)]
pub struct StackTebFields {
    pub StackTop: usize,
    pub StackBottom: usize,
    pub StackBottomPlusGuard: usize,
    pub GuaranteedStackBytes: usize,
}

/// A mutable reference to a stack can be used as a stack. The lifetime of the
/// resulting fiber will be bound to that of the reference.
unsafe impl<'a, S: FiberStack> FiberStack for &'a mut S {
    #[inline]
    fn top(&self) -> StackPointer {
        (**self).top()
    }

    #[inline]
    fn bottom(&self) -> StackPointer {
        (**self).bottom()
    }

    #[inline]
    #[cfg(windows)]
    fn teb_fields(&self) -> StackTebFields {
        (**self).teb_fields()
    }

    #[inline]
    #[cfg(windows)]
    fn update_teb_fields(&mut self, stack_limit: usize, guaranteed_stack_bytes: usize) {
        (**self).update_teb_fields(stack_limit, guaranteed_stack_bytes)
    }
}
