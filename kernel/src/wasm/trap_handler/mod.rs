// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod backtrace;

use crate::arch;
use crate::wasm::runtime::{CodeMemory, StaticVMOffsets, VMContext};
use crate::wasm::trap_handler::backtrace::Backtrace;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use core::cell::{Cell, RefCell, UnsafeCell};
use core::mem::MaybeUninit;
use core::ptr;
use sync::{OnceLock, RwLock};
use thread_local::thread_local;

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
        let r = unsafe { arch::setjmp(state.jmp_buf.as_ptr()) };
        if r == 0isize {
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
        /// The address of the inaccessible data or zero if trap wasn't caused by data access.
        faulting_addr: usize,
        /// The trap code associated with this trap.
        trap: crate::wasm::trap::Trap,
    },
}

enum UnwindReason {
    // TODO reenable for host functions
    // Panic(Box<dyn std::any::Any + Send>),
    Trap(TrapReason),
}

thread_local! {
    static CURRENT_STATE: Cell<Option<*const CallThreadState>> = Cell::new(None);
}

pub struct CallThreadState {
    unwind: UnsafeCell<MaybeUninit<(UnwindReason, Option<Backtrace>)>>,
    pub jmp_buf: RefCell<arch::JmpBufStruct>,
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
                jmp_buf: RefCell::new(arch::JmpBufStruct::default()),
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
        closure: impl FnOnce(&Self) -> isize,
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
        unsafe { (*self.unwind.get()).as_ptr().read() }
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
            arch::longjmp(self.jmp_buf.as_ptr(), 1);
        }
    }

    pub(crate) unsafe fn set_jit_trap(
        &self,
        pc: usize,
        fp: usize,
        faulting_addr: usize,
        trap: crate::wasm::trap::Trap,
    ) {
        let backtrace = unsafe { Backtrace::new_with_trap_state(self, Some((pc, fp))) };
        // Safety: `MaybeUninit` ensures proper alignment.
        unsafe {
            (*self.unwind.get()).as_mut_ptr().write((
                UnwindReason::Trap(TrapReason::Jit {
                    pc,
                    faulting_addr,
                    trap,
                }),
                Some(backtrace),
            ));
        }
    }

    /// Get the previous `CallThreadState`.
    pub fn prev(&self) -> *const CallThreadState {
        self.prev.get()
    }

    #[inline]
    pub(crate) fn push(&self) {
        assert!(self.prev.get().is_null());
        self.prev.set(
            CURRENT_STATE
                .replace(Some(ptr::from_ref(self)))
                .unwrap_or(ptr::null_mut()),
        );
    }

    #[inline]
    pub(crate) fn pop(&self) {
        let prev = self.prev.replace(ptr::null());
        let head = CURRENT_STATE.replace(Some(prev)).unwrap_or(ptr::null_mut());
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

pub fn trap_handler(pc: usize, fp: usize, faulting_addr: usize) -> Result<(), !> {
    if let Some(state) = CURRENT_STATE.get() {
        let state = unsafe { &*state };

        // If this fault wasn't in wasm code, then it's not our problem
        let Some((code, text_offset)) = lookup_code(pc) else {
            return Ok(());
        };

        // If we don't have a trap code for this offset, that is bad, and it means
        // we messed up somehow
        let Some(trap) = code.lookup_trap_code(text_offset) else {
            panic!("no trap code for text offset {text_offset}");
            // return Ok(());
        };

        // Save the trap information into our thread local
        unsafe {
            state.set_jit_trap(pc, fp, faulting_addr, trap);
        }

        // And finally do the longjmp back to the last `catch_trap` that we know of
        unsafe {
            arch::longjmp(state.jmp_buf.as_ptr(), 1);
        }
    }

    // If no wasm code is executing, we don't handle this as a wasm
    // trap.
    Ok(())
}

fn global_code() -> &'static RwLock<GlobalRegistry> {
    static GLOBAL_CODE: OnceLock<RwLock<GlobalRegistry>> = OnceLock::new();
    GLOBAL_CODE.get_or_init(Default::default)
}

type GlobalRegistry = BTreeMap<usize, (usize, Arc<CodeMemory>)>;

/// Find which registered region of code contains the given program counter, and
/// what offset that PC is within that module's code.
pub fn lookup_code(pc: usize) -> Option<(Arc<CodeMemory>, usize)> {
    let all_modules = global_code().read();

    let (_end, (start, module)) = all_modules.range(pc..).next()?;
    let text_offset = pc.checked_sub(*start)?;
    Some((module.clone(), text_offset))
}

/// Registers a new region of code.
///
/// Must not have been previously registered and must be `unregister`'d to
/// prevent leaking memory.
///
/// This is used by trap handling to determine which region of code a faulting
/// address.
pub fn register_code(code: &Arc<CodeMemory>) {
    let text = code.text();
    if text.is_empty() {
        return;
    }
    let start = text.as_ptr() as usize;
    let end = start + text.len() - 1;
    let prev = global_code().write().insert(end, (start, code.clone()));
    assert!(prev.is_none());
}
