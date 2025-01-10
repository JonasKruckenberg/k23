use crate::wasm::runtime::{StaticVMOffsets, VMContext};
pub use backtrace::Backtrace;
use core::cell::{Cell, UnsafeCell};
use core::mem::MaybeUninit;
use core::ptr;

mod backtrace;

pub fn raise_trap(reason: TrapReason) {
    // Safety: TLS storage is always initialized
    let state = unsafe { &*TLS.get().unwrap() };
    state.unwind_with(UnwindReason::Trap(reason))
}

pub fn catch_traps<F>(
    caller: *mut VMContext,
    vmctx_plan: StaticVMOffsets,
    mut closure: F,
) -> Result<(), Trap>
where
    F: FnMut(*mut VMContext),
{
    let result = CallThreadState::new(caller, vmctx_plan).with(|state| {
        // Safety: call to extern
        let r = unsafe { crate::wasm::placeholder::setjmp::setjmp(state.jmp_buf.as_ptr().cast()) };
        if r == 0i32 {
            closure(caller);
        }
        r
    });

    match result {
        Ok(x) => Ok(x),
        Err((UnwindReason::Trap(reason), backtrace)) => Err(Trap { reason, backtrace }),
        // Err((UnwindReason::Panic(panic), _)) => std::panic::resume_unwind(panic),
    }
}

/// Stores trace message with backtrace.
#[derive(Debug)]
pub struct Trap {
    /// Original reason from where this trap originated.
    pub reason: TrapReason,
    /// Wasm backtrace of the trap, if any.
    pub backtrace: Option<Backtrace>,
}

/// Enumeration of different methods of raising a trap.
#[derive(Debug)]
pub enum TrapReason {
    /// A trap raised from a wasm builtin
    Wasm(crate::wasm::trap::Trap),
    /// A trap raised from Cranelift-generated code.
    Jit {
        /// The program counter where this trap originated.
        ///
        /// This is later used with side tables from compilation to translate
        /// the trapping address to a trap code.
        pc: usize,
        /// If the trap was a memory-related trap such as SIGSEGV then this
        /// field will contain the address of the inaccessible data.
        ///
        /// Note that wasm loads/stores are not guaranteed to fill in this
        /// information. Dynamically-bounds-checked memories, for example, will
        /// not access an invalid address but may instead load from NULL or may
        /// explicitly jump to a `ud2` instruction.
        faulting_addr: Option<usize>,
        /// The trap code associated with this trap.
        trap: crate::wasm::trap::Trap,
    },
}

enum UnwindReason {
    // TODO reenable for host functions
    // Panic(Box<dyn std::any::Any + Send>),
    Trap(TrapReason),
}

#[thread_local]
pub static TLS: Cell<Option<*const CallThreadState>> = Cell::new(None);

pub struct CallThreadState {
    unwind: UnsafeCell<MaybeUninit<(UnwindReason, Option<Backtrace>)>>,
    pub jmp_buf: Cell<crate::wasm::placeholder::setjmp::jmp_buf>,
    offsets: StaticVMOffsets,
    vmctx: *mut VMContext,
    prev: Cell<*const CallThreadState>,
    /// The values of `VMRuntimeLimits::last_wasm_{exit_{pc,fp},entry_sp}`
    /// for the *previous* `CallThreadState` for this same store/limits. Our
    /// *current* last wasm PC/FP/SP are saved in `self.limits`. We save a
    /// copy of the old registers here because the `VMContext` fields
    /// typically don't change across nested calls into Wasm (i.e. they are
    /// typically calls back into the same store and `self.limits ==
    /// self.prev.limits`) and we must to maintain the list of
    /// contiguous-Wasm-frames stack regions for backtracing purposes.
    old_last_wasm_exit_fp: Cell<usize>,
    old_last_wasm_exit_pc: Cell<usize>,
    old_last_wasm_entry_fp: Cell<usize>,
}

impl CallThreadState {
    pub fn new(vmctx: *mut VMContext, vmoffsets: StaticVMOffsets) -> Self {
        // Safety: the offsets below are small so the code *should* not overflow
        // TODO this is horrific
        unsafe {
            Self {
                unwind: UnsafeCell::new(MaybeUninit::uninit()),
                jmp_buf: Cell::new(crate::wasm::placeholder::setjmp::jmp_buf::from([0; 48])),
                vmctx,
                prev: Cell::new(ptr::null()),
                old_last_wasm_exit_fp: Cell::new(
                    *vmctx
                        .byte_add(vmoffsets.vmctx_last_wasm_exit_fp() as usize)
                        .cast::<usize>(),
                ),
                old_last_wasm_exit_pc: Cell::new(
                    *vmctx
                        .byte_add(vmoffsets.vmctx_last_wasm_exit_pc() as usize)
                        .cast::<usize>(),
                ),
                old_last_wasm_entry_fp: Cell::new(
                    *vmctx
                        .byte_add(vmoffsets.vmctx_last_wasm_entry_fp() as usize)
                        .cast::<usize>(),
                ),
                offsets: vmoffsets,
            }
        }
    }

    fn with(
        self,
        closure: impl FnOnce(&Self) -> i32,
    ) -> Result<(), (UnwindReason, Option<Backtrace>)> {
        struct Reset<'a> {
            state: &'a CallThreadState,
        }

        impl Drop for Reset<'_> {
            #[inline]
            fn drop(&mut self) {
                self.state.pop();
            }
        }

        let ret = {
            self.push();
            let reset = Reset { state: &self };
            closure(reset.state)
        };

        if ret == 0 {
            Ok(())
        } else {
            // Safety: a non-null ret code means the implementation has also written to the `unwind` field.
            Err(unsafe { self.read_unwind() })
        }
    }

    #[cold]
    unsafe fn read_unwind(&self) -> (UnwindReason, Option<Backtrace>) {
        (*self.unwind.get()).as_ptr().read()
    }

    fn unwind_with(&self, reason: UnwindReason) -> ! {
        let backtrace = match reason {
            // Safety: since we pass None to `new_with_trap_state`, pc and fp will be read from the
            // `VMContext` instead. We have to trust that those are valid.
            UnwindReason::Trap(_) => unsafe { Some(Backtrace::new_with_trap_state(self, None)) },
            // UnwindReason::Panic(_) => None,
        };

        // Safety: `MaybeUninit` ensures proper alignment.
        unsafe {
            (*self.unwind.get()).as_mut_ptr().write((reason, backtrace));
        }

        // Safety: call to extern
        unsafe {
            crate::wasm::placeholder::setjmp::longjmp(self.jmp_buf.as_ptr().cast(), 1);
        }
    }

    pub(crate) unsafe fn set_jit_trap(
        &self,
        pc: usize,
        fp: usize,
        faulting_addr: Option<usize>,
        trap: crate::wasm::trap::Trap,
    ) {
        let backtrace = Backtrace::new_with_trap_state(self, Some((pc, fp)));
        // Safety: `MaybeUninit` ensures proper alignment.
        (*self.unwind.get()).as_mut_ptr().write((
            UnwindReason::Trap(TrapReason::Jit {
                pc,
                faulting_addr,
                trap,
            }),
            Some(backtrace),
        ));
    }

    /// Get the previous `CallThreadState`.
    pub fn prev(&self) -> *const CallThreadState {
        self.prev.get()
    }

    #[inline]
    pub(crate) fn push(&self) {
        assert!(self.prev.get().is_null());
        self.prev.set(
            TLS.replace(Some(ptr::from_ref(self)))
                .unwrap_or(ptr::null_mut()),
        );
    }

    #[inline]
    pub(crate) fn pop(&self) {
        let prev = self.prev.replace(ptr::null());
        let head = TLS.replace(Some(prev)).unwrap_or(ptr::null_mut());
        assert!(ptr::eq(head, self));
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Self> {
        let mut state = Some(self);
        core::iter::from_fn(move || {
            let this = state?;
            // Safety: `prev` is always either a null ptr (indicating the end of the list) or a valid pointer to a `CallThreadState`.
            // This is ensured by the `push` method.
            state = unsafe { this.prev().as_ref() };
            Some(this)
        })
    }
}

impl Drop for CallThreadState {
    fn drop(&mut self) {
        // Safety: offsets are small so the code below *should* overflow
        // FIXME this is horrific
        unsafe {
            *self
                .vmctx
                .byte_add(self.offsets.vmctx_last_wasm_exit_fp() as usize)
                .cast::<usize>() = self.old_last_wasm_exit_fp.get();
            *self
                .vmctx
                .byte_add(self.offsets.vmctx_last_wasm_exit_pc() as usize)
                .cast::<usize>() = self.old_last_wasm_exit_pc.get();
            *self
                .vmctx
                .byte_add(self.offsets.vmctx_last_wasm_entry_fp() as usize)
                .cast::<usize>() = self.old_last_wasm_entry_fp.get();
        }
    }
}
