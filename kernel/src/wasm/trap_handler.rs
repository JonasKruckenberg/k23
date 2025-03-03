// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch;
use crate::vm::VirtualAddress;
use crate::wasm::runtime::{StaticVMOffsets, VMContext, code_registry};
use crate::wasm::{Error, Trap};
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;
use core::mem::ManuallyDrop;
use core::ops::ControlFlow;
use core::ptr;
use core::ptr::{NonNull, addr_of_mut};
use core::range::Range;
use core::slice::SliceIndex;
use cpu_local::cpu_local;

cpu_local! {
    static ACTIVATION: Cell<*mut Activation> = Cell::new(ptr::null_mut())
}

#[derive(Debug)]
pub enum TrapReason {
    /// A trap raised from a wasm builtin
    Wasm(Trap),
    /// A trap raised from Cranelift-generated code.
    Jit {
        /// The program counter where this trap originated.
        ///
        /// This is later used with side tables from compilation to translate
        /// the trapping address to a trap code.
        pc: VirtualAddress,
        /// If the trap was a memory-related trap such as SIGSEGV then this
        /// field will contain the address of the inaccessible data.
        ///
        /// Note that wasm loads/stores are not guaranteed to fill in this
        /// information. Dynamically-bounds-checked memories, for example, will
        /// not access an invalid address but may instead load from NULL or may
        /// explicitly jump to a `ud2` instruction.
        faulting_addr: VirtualAddress,
        /// The trap code associated with this trap.
        trap: Trap,
    },
}

enum UnwindReason {
    // TODO reenable for host functions
    // Panic(Box<dyn std::any::Any + Send>),
    Trap(TrapReason),
}

pub struct Activation {
    unwind: Cell<Option<(UnwindReason, Option<RawBacktrace>)>>,
    jmp_buf: arch::JmpBuf,
    prev: *mut Activation,
    async_guard_range: Range<*mut u8>,

    vmctx: *mut VMContext,
    vmoffsets: StaticVMOffsets,

    // The values of `VMRuntimeLimits::last_wasm_{exit_{pc,fp},entry_sp}`
    // for the *previous* `CallThreadState` for this same store/limits. Our
    // *current* last wasm PC/FP/SP are saved in `self.limits`. We save a
    // copy of the old registers here because the `VMRuntimeLimits`
    // typically doesn't change across nested calls into Wasm (i.e. they are
    // typically calls back into the same store and `self.limits ==
    // self.prev.limits`) and we must to maintain the list of
    // contiguous-Wasm-frames stack regions for backtracing purposes.
    old_last_wasm_exit_fp: Cell<VirtualAddress>,
    old_last_wasm_exit_pc: Cell<VirtualAddress>,
    old_last_wasm_entry_fp: Cell<VirtualAddress>,
}

impl Activation {
    pub fn new(
        vmctx: *mut VMContext,
        vmoffsets: StaticVMOffsets,
        jmp_buf: &arch::JmpBufStruct,
    ) -> Self {
        Self {
            unwind: Cell::new(None),
            jmp_buf: ptr::from_ref(jmp_buf),
            prev: ACTIVATION.get(),
            async_guard_range: Range::from(ptr::null_mut()..ptr::null_mut()), // TODO

            #[expect(clippy::undocumented_unsafe_blocks, reason = "")]
            old_last_wasm_exit_fp: Cell::new(unsafe {
                *vmctx
                    .byte_add(vmoffsets.vmctx_last_wasm_exit_fp() as usize)
                    .cast::<VirtualAddress>()
            }),
            #[expect(clippy::undocumented_unsafe_blocks, reason = "")]
            old_last_wasm_exit_pc: Cell::new(unsafe {
                *vmctx
                    .byte_add(vmoffsets.vmctx_last_wasm_exit_pc() as usize)
                    .cast::<VirtualAddress>()
            }),
            #[expect(clippy::undocumented_unsafe_blocks, reason = "")]
            old_last_wasm_entry_fp: Cell::new(unsafe {
                *vmctx
                    .byte_add(vmoffsets.vmctx_last_wasm_entry_fp() as usize)
                    .cast::<VirtualAddress>()
            }),

            vmctx,
            vmoffsets,
        }
    }

    fn iter(&self) -> impl Iterator<Item = &Self> {
        let mut state = Some(self);
        core::iter::from_fn(move || {
            let this = state?;
            // Safety: `prev` is always either a null ptr (indicating the end of the list) or a valid pointer to a `CallThreadState`.
            // This is ensured by the `push` method.
            state = unsafe { this.prev.as_ref() };
            Some(this)
        })
    }
}

impl Drop for Activation {
    fn drop(&mut self) {
        // FIXME this is horrific
        // Safety: offsets are small so the code below *shouldn't* overflow
        unsafe {
            *self
                .vmctx
                .byte_add(self.vmoffsets.vmctx_last_wasm_exit_fp() as usize)
                .cast::<VirtualAddress>() = self.old_last_wasm_exit_fp.get();
            *self
                .vmctx
                .byte_add(self.vmoffsets.vmctx_last_wasm_exit_pc() as usize)
                .cast::<VirtualAddress>() = self.old_last_wasm_exit_pc.get();
            *self
                .vmctx
                .byte_add(self.vmoffsets.vmctx_last_wasm_entry_fp() as usize)
                .cast::<VirtualAddress>() = self.old_last_wasm_entry_fp.get();
        }
    }
}

pub fn raise_trap(reason: TrapReason) {
    #[expect(clippy::undocumented_unsafe_blocks, reason = "")]
    let activation = unsafe { ACTIVATION.get().as_ref().unwrap() };

    // record the unwind details
    let backtrace = RawBacktrace::new(activation, None);
    activation
        .unwind
        .set(Some((UnwindReason::Trap(reason), Some(backtrace))));

    // longjmp back to Rust
    #[expect(clippy::undocumented_unsafe_blocks, reason = "")]
    unsafe {
        arch::longjmp(activation.jmp_buf, 1);
    }
}

pub fn handle_wasm_exception(
    pc: VirtualAddress,
    fp: VirtualAddress,
    faulting_addr: VirtualAddress,
) -> ControlFlow<()> {
    if let Some(activation) = NonNull::new(ACTIVATION.get()) {
        let Some((code, text_offset)) = code_registry::lookup_code(pc.get()) else {
            tracing::debug!("no JIT code registered for pc {pc}");
            return ControlFlow::Continue(());
        };

        let Some(trap) = code.lookup_trap_code(text_offset) else {
            tracing::debug!("no JIT trap registered for pc {pc}");
            return ControlFlow::Continue(());
        };

        #[expect(clippy::undocumented_unsafe_blocks, reason = "")]
        let activation = unsafe { activation.as_ref() };

        // record the unwind details
        let backtrace = RawBacktrace::new(activation, Some((pc, fp)));
        activation.unwind.set(Some((
            UnwindReason::Trap(TrapReason::Jit {
                pc,
                faulting_addr,
                trap,
            }),
            Some(backtrace),
        )));

        // longjmp back to Rust
        #[expect(clippy::undocumented_unsafe_blocks, reason = "")]
        unsafe {
            arch::longjmp(activation.jmp_buf, 1);
        }
    } else {
        // ACTIVATION is a nullptr
        //  => means no activations on stack
        //  => means exception cannot be a WASM trap
        ControlFlow::Continue(())
    }
}

pub fn catch_traps<F>(caller: *mut VMContext, vmoffsets: StaticVMOffsets, f: F) -> Result<(), Error>
where
    F: FnOnce(),
{
    let mut prev_state = ptr::null_mut();
    let ret_code = arch::call_with_setjmp(|jmp_buf| {
        let mut activation = Activation::new(caller, vmoffsets, jmp_buf);

        prev_state = ACTIVATION.replace(ptr::from_mut(&mut activation).cast());
        f();

        0_i32
    });

    if ret_code == 0 {
        ACTIVATION.set(prev_state);

        Ok(())
    } else {
        #[expect(clippy::undocumented_unsafe_blocks, reason = "")]
        let (unwind_reason, backtrace) = unsafe { ACTIVATION.get().as_ref() }
            .unwrap()
            .unwind
            .take()
            .unwrap();
        ACTIVATION.set(prev_state);

        match unwind_reason {
            UnwindReason::Trap(TrapReason::Wasm(trap)) => Err(Error::Trap {
                trap,
                message: "WASM builtin trapped".to_string(),
                backtrace,
            }),
            UnwindReason::Trap(TrapReason::Jit { trap, .. }) => Err(Error::Trap {
                trap,
                message: "WASM JIT code trapped".to_string(),
                backtrace,
            }),
        }
    }
}

#[derive(Debug)]
pub struct RawBacktrace(Vec<Frame>);

/// A stack frame within a Wasm stack trace.
#[derive(Debug)]
pub struct Frame {
    pub pc: VirtualAddress,
    pub fp: VirtualAddress,
}

impl RawBacktrace {
    fn new(
        activation: &Activation,
        trap_pc_and_fp: Option<(VirtualAddress, VirtualAddress)>,
    ) -> Self {
        let mut frames = vec![];
        Self::trace_with_trap_state(activation, trap_pc_and_fp, |frame| {
            frames.push(frame);
            ControlFlow::Continue(())
        });
        Self(frames)
    }

    /// Walk the current Wasm stack, calling `f` for each frame we walk.
    pub(crate) fn trace_with_trap_state(
        activation: &Activation,
        trap_pc_and_fp: Option<(VirtualAddress, VirtualAddress)>,
        mut f: impl FnMut(Frame) -> ControlFlow<()>,
    ) {
        tracing::trace!("====== Capturing Backtrace ======");

        // If we exited Wasm by catching a trap, then the Wasm-to-host
        // trampoline did not get a chance to save the last Wasm PC and FP,
        // and we need to use the plumbed-through values instead.
        #[expect(clippy::undocumented_unsafe_blocks, reason = "")]
        let (last_wasm_exit_pc, last_wasm_exit_fp) = trap_pc_and_fp.unwrap_or_else(|| unsafe {
            // TODO this is horrible can we improve this?
            let pc = *activation
                .vmctx
                .byte_add(activation.vmoffsets.vmctx_last_wasm_exit_pc() as usize)
                .cast::<VirtualAddress>();
            let fp = *activation
                .vmctx
                .byte_add(activation.vmoffsets.vmctx_last_wasm_entry_fp() as usize)
                .cast::<VirtualAddress>();

            (pc, fp)
        });

        #[expect(clippy::undocumented_unsafe_blocks, reason = "")]
        let last_wasm_entry_fp = unsafe {
            *activation
                .vmctx
                .byte_add(activation.vmoffsets.vmctx_last_wasm_entry_fp() as usize)
                .cast::<VirtualAddress>()
        };

        let activations =
            core::iter::once((last_wasm_exit_pc, last_wasm_exit_fp, last_wasm_entry_fp))
                .chain(activation.iter().map(|state| {
                    (
                        state.old_last_wasm_exit_pc.get(),
                        state.old_last_wasm_exit_fp.get(),
                        state.old_last_wasm_entry_fp.get(),
                    )
                }))
                .take_while(|&(pc, fp, sp)| {
                    if pc.get() == 0 {
                        debug_assert_eq!(fp.get(), 0);
                        debug_assert_eq!(sp.get(), 0);
                    }
                    pc.get() != 0
                });

        for (pc, fp, sp) in activations {
            if let ControlFlow::Break(()) = Self::trace_through_wasm(pc, fp, sp, &mut f) {
                tracing::trace!("====== Done Capturing Backtrace (closure break) ======");
                return;
            }
        }

        tracing::trace!("====== Done Capturing Backtrace (reached end of activations) ======");
    }

    /// Walk through a contiguous sequence of Wasm frames starting with the
    /// frame at the given PC and FP and ending at `trampoline_sp`.
    fn trace_through_wasm(
        mut pc: VirtualAddress,
        mut fp: VirtualAddress,
        trampoline_fp: VirtualAddress,
        mut f: impl FnMut(Frame) -> ControlFlow<()>,
    ) -> ControlFlow<()> {
        f(Frame { pc, fp })?;

        tracing::trace!("=== Tracing through contiguous sequence of Wasm frames ===");
        tracing::trace!("trampoline_fp = {trampoline_fp}");
        tracing::trace!("   initial pc = {pc}");
        tracing::trace!("   initial fp = {fp}");

        // We already checked for this case in the `trace_with_trap_state`
        // caller.
        assert_ne!(pc.get(), 0);
        assert!(pc.is_canonical());
        assert_ne!(fp.get(), 0);
        assert!(fp.is_canonical());
        assert_ne!(trampoline_fp.get(), 0);
        assert!(trampoline_fp.is_canonical());

        // This loop will walk the linked list of frame pointers starting at
        // `fp` and going up until `trampoline_fp`. We know that both `fp` and
        // `trampoline_fp` are "trusted values" aka generated and maintained by
        // Cranelift. This means that it should be safe to walk the linked list
        // of pointers and inspect wasm frames.
        //
        // Note, though, that any frames outside of this range are not
        // guaranteed to have valid frame pointers. For example native code
        // might be using the frame pointer as a general purpose register. Thus
        // we need to be careful to only walk frame pointers in this one
        // contiguous linked list.
        //
        // To know when to stop iteration all architectures' stacks currently
        // look something like this:
        //
        //     | ...               |
        //     | Native Frames     |
        //     | ...               |
        //     |-------------------|
        //     | ...               | <-- Trampoline FP            |
        //     | Trampoline Frame  |                              |
        //     | ...               | <-- Trampoline SP            |
        //     |-------------------|                            Stack
        //     | Return Address    |                            Grows
        //     | Previous FP       | <-- Wasm FP                Down
        //     | ...               |                              |
        //     | Wasm Frames       |                              |
        //     | ...               |                              V
        //
        // The trampoline records its own frame pointer (`trampoline_fp`),
        // which is guaranteed to be above all Wasm. To check when we've
        // reached the trampoline frame, it is therefore sufficient to
        // check when the next frame pointer is equal to `trampoline_fp`. Once
        // that's hit then we know that the entire linked list has been
        // traversed.
        //
        // Note that it might be possible that this loop doesn't execute at all.
        // For example if the entry trampoline called wasm which `return_call`'d
        // an imported function which is an exit trampoline, then
        // `fp == trampoline_fp` on the entry of this function, meaning the loop
        // won't actually execute anything.
        while fp != trampoline_fp {
            // At the start of each iteration of the loop, we know that `fp` is
            // a frame pointer from Wasm code. Therefore, we know it is not
            // being used as an extra general-purpose register, and it is safe
            // dereference to get the PC and the next older frame pointer.
            //
            // The stack also grows down, and therefore any frame pointer we are
            // dealing with should be less than the frame pointer on entry to
            // Wasm. Finally also assert that it's aligned correctly as an
            // additional sanity check.
            assert!(trampoline_fp > fp, "{trampoline_fp} > {fp}");
            arch::assert_fp_is_aligned(fp);

            tracing::trace!("--- Tracing through one Wasm frame ---");
            tracing::trace!("pc = {pc}");
            tracing::trace!("fp = {fp}");

            f(Frame { pc, fp })?;

            #[expect(clippy::undocumented_unsafe_blocks, reason = "")]
            unsafe {
                pc = arch::get_next_older_pc_from_fp(fp);
            }

            // We rely on this offset being zero for all supported architectures
            // in `crates/cranelift/src/component/compiler.rs` when we set the
            // Wasm exit FP. If this ever changes, we will need to update that
            // code as well!
            assert_eq!(arch::NEXT_OLDER_FP_FROM_FP_OFFSET, 0);

            // Get the next older frame pointer from the current Wasm frame
            // pointer.
            #[expect(clippy::undocumented_unsafe_blocks, reason = "")]
            #[expect(clippy::cast_ptr_alignment, reason = "")]
            let next_older_fp = unsafe {
                *fp.as_mut_ptr()
                    .cast::<VirtualAddress>()
                    .add(arch::NEXT_OLDER_FP_FROM_FP_OFFSET)
            };

            // Because the stack always grows down, the older FP must be greater
            // than the current FP.
            assert!(next_older_fp > fp, "{next_older_fp} > {fp}");
            fp = next_older_fp;
        }

        tracing::trace!("=== Done tracing contiguous sequence of Wasm frames ===");
        ControlFlow::Continue(())
    }

    /// Iterate over the frames inside this backtrace.
    pub fn frames(&self) -> impl ExactSizeIterator<Item = &Frame> + DoubleEndedIterator {
        self.0.iter()
    }
}
