// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch;
use crate::mem::VirtualAddress;
use crate::wasm::TrapKind;
use crate::wasm::code_registry::lookup_code;
use crate::wasm::store::StoreOpaque;
use crate::wasm::vm::{VMContext, VMStoreContext};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::Cell;
use core::num::NonZeroU32;
use core::ops::ControlFlow;
use core::panic::AssertUnwindSafe;
use core::ptr::NonNull;
use core::{fmt, ptr};
use cpu_local::cpu_local;

/// Description about a fault that occurred in WebAssembly.
#[derive(Debug)]
pub struct WasmFault {
    /// The size of memory, in bytes, at the time of the fault.
    pub memory_size: usize,
    /// The WebAssembly address at which the fault occurred.
    pub wasm_address: u64,
}

impl fmt::Display for WasmFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "memory fault at wasm address 0x{:x} in linear memory of size 0x{:x}",
            self.wasm_address, self.memory_size,
        )
    }
}

#[derive(Debug)]
pub struct Trap {
    /// Original reason from where this trap originated.
    pub reason: TrapReason,
    /// Wasm backtrace of the trap, if any.
    pub backtrace: Option<RawBacktrace>,
    // The Wasm Coredump, if any.
    // pub coredumpstack: Option<CoreDumpStack>,
}

#[derive(Debug)]
pub enum TrapReason {
    /// A user-raised trap through `raise_user_trap`.
    User(anyhow::Error),

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
        /// explicitly jump to a `ud2` instruction. This is only available for
        /// fault-based traps which are one of the main ways, but not the only
        /// way, to run wasm.
        faulting_addr: Option<VirtualAddress>,

        /// The trap code associated with this trap.
        trap: TrapKind,
    },

    /// A trap raised from a wasm builtin
    Wasm(TrapKind),
}

impl From<anyhow::Error> for TrapReason {
    fn from(err: anyhow::Error) -> Self {
        TrapReason::User(err)
    }
}

impl From<TrapKind> for TrapReason {
    fn from(code: TrapKind) -> Self {
        TrapReason::Wasm(code)
    }
}

pub enum UnwindReason {
    Panic(Box<dyn core::any::Any + Send>),
    Trap(TrapReason),
}

pub(in crate::wasm) unsafe fn raise_preexisting_trap() -> ! {
    // Safety: ensured by caller
    unsafe {
        let activation = ACTIVATION.get().as_ref().unwrap();
        activation.unwind()
    }
}

pub fn catch_traps<F>(store: &mut StoreOpaque, f: F) -> Result<(), Trap>
where
    F: FnOnce(NonNull<VMContext>),
{
    let caller = store.default_caller();
    let mut prev_state = ptr::null_mut();
    let ret_code = arch::call_with_setjmp(|jmp_buf| {
        let mut activation = Activation::new(store, jmp_buf);

        prev_state = ACTIVATION.replace(ptr::from_mut(&mut activation).cast());
        f(caller);

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
            UnwindReason::Trap(reason) => Err(Trap { reason, backtrace }),
            UnwindReason::Panic(payload) => crate::panic::resume_unwind(payload),
        }
    }
}

cpu_local! {
    static ACTIVATION: Cell<*mut Activation> = Cell::new(ptr::null_mut())
}

/// ```text
/// ┌─────────────────────┐◄───── highest, or oldest, stack address
/// │ native stack frames │
/// │         ...         │
/// │  ┌───────────────┐◄─┼──┐
/// │  │   Activation  │  │  │
/// │  └───────────────┘  │  p
/// ├─────────────────────┤  r
/// │  wasm stack frames  │  e
/// │         ...         │  v
/// ├─────────────────────┤  │
/// │ native stack frames │  │
/// │         ...         │  │
/// │  ┌───────────────┐◄─┼──┼── TLS pointer
/// │  │   Activation  ├──┼──┘
/// │  └───────────────┘  │
/// ├─────────────────────┤
/// │  wasm stack frames  │
/// │         ...         │
/// ├─────────────────────┤
/// │ native stack frames │
/// │         ...         │
/// └─────────────────────┘◄───── smallest, or youngest, stack address
/// ```
pub struct Activation {
    unwind: Cell<Option<(UnwindReason, Option<RawBacktrace>)>>,
    jmp_buf: arch::JmpBuf,
    prev: Cell<*mut Activation>,
    vm_store_context: NonNull<VMStoreContext>,

    // async_guard_range: Range<*mut u8>,

    // vmctx: *mut VMContext,
    // vmoffsets: VMOffsets,

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
    pub fn new(store: &mut StoreOpaque, jmp_buf: &arch::JmpBufStruct) -> Self {
        Self {
            unwind: Cell::new(None),
            jmp_buf: ptr::from_ref(jmp_buf),
            prev: Cell::new(ACTIVATION.get()),

            vm_store_context: store.vm_store_context_ptr(),
            old_last_wasm_exit_fp: Cell::new(unsafe {
                *store.vm_store_context().last_wasm_exit_fp.get()
            }),
            old_last_wasm_exit_pc: Cell::new(unsafe {
                *store.vm_store_context().last_wasm_exit_pc.get()
            }),
            old_last_wasm_entry_fp: Cell::new(unsafe {
                *store.vm_store_context().last_wasm_entry_fp.get()
            }),
        }
    }

    fn iter(&self) -> impl Iterator<Item = &Self> {
        let mut state = Some(self);
        core::iter::from_fn(move || {
            let this = state?;
            // Safety: `prev` is always either a null ptr (indicating the end of the list) or a valid pointer to a `CallThreadState`.
            // This is ensured by the `push` method.
            state = unsafe { this.prev.get().as_ref() };
            Some(this)
        })
    }

    #[inline]
    pub(crate) unsafe fn push(&mut self) {
        assert!(self.prev.get().is_null());
        let prev = ACTIVATION.replace(ptr::from_mut(self));
        self.prev.set(prev);
    }

    #[inline]
    pub(crate) unsafe fn pop(&self) {
        let prev = self.prev.replace(ptr::null_mut());
        let head = ACTIVATION.replace(prev);
        assert!(ptr::eq(head, self));
    }

    #[cold]
    fn read_unwind(&self) -> (UnwindReason, Option<RawBacktrace>) {
        self.unwind.replace(None).unwrap()
    }

    fn record_unwind(&self, reason: UnwindReason) {
        if cfg!(debug_assertions) {
            let prev = self.unwind.replace(None);
            assert!(prev.is_none());
        }
        let backtrace = match &reason {
            // Panics don't need backtraces. There is nowhere to attach the
            // hypothetical backtrace to and it doesn't really make sense to try
            // in the first place since this is a Rust problem rather than a
            // Wasm problem.
            UnwindReason::Panic(_) => None,
            // // And if we are just propagating an existing trap that already has
            // // a backtrace attached to it, then there is no need to capture a
            // // new backtrace either.
            // UnwindReason::Trap(TrapReason::User(err))
            // if err.downcast_ref::<RawBacktrace>().is_some() =>
            //     {
            //         (None, None)
            //     }
            UnwindReason::Trap(_) => self.capture_backtrace(self.vm_store_context.as_ptr(), None),
            // self.capture_coredump(self.vm_store_context.as_ptr(), None),
        };
        self.unwind.set(Some((reason, backtrace)));
    }

    unsafe fn unwind(&self) -> ! {
        // Safety: ensured by caller
        unsafe {
            debug_assert!(!self.jmp_buf.is_null());
            arch::longjmp(self.jmp_buf, 1);
        }
    }

    fn capture_backtrace(
        &self,
        vm_store_context: *mut VMStoreContext,
        trap_pc_and_fp: Option<(VirtualAddress, VirtualAddress)>,
    ) -> Option<RawBacktrace> {
        let backtrace = RawBacktrace::new(vm_store_context, self, trap_pc_and_fp);
        Some(backtrace)
    }
}

impl Drop for Activation {
    fn drop(&mut self) {
        // Unwind information should not be present as it should have
        // already been processed.
        debug_assert!(self.unwind.replace(None).is_none());

        unsafe {
            let cx = self.vm_store_context.as_ref();
            *cx.last_wasm_exit_fp.get() = self.old_last_wasm_exit_fp.get();
            *cx.last_wasm_exit_pc.get() = self.old_last_wasm_exit_pc.get();
            *cx.last_wasm_entry_fp.get() = self.old_last_wasm_entry_fp.get();
        }
    }
}

pub fn catch_unwind_and_record_trap<R>(f: impl FnOnce() -> R) -> R::Abi
where
    R: HostResult,
{
    let (ret, unwind) = R::maybe_catch_unwind(f);
    if let Some(unwind) = unwind {
        let activation = unsafe { ACTIVATION.get().as_ref().unwrap() };
        activation.record_unwind(unwind);
    }

    ret
}

/// A trait used in conjunction with `catch_unwind_and_record_trap` to convert a
/// Rust-based type to a specific ABI while handling traps/unwinds.
pub trait HostResult {
    /// The type of the value that's returned to Cranelift-compiled code. Needs
    /// to be ABI-safe to pass through an `extern "C"` return value.
    type Abi: Copy;
    /// This type is implemented for return values from host function calls and
    /// builtins. The `Abi` value of this trait represents either a successful
    /// execution with some payload state or that a failed execution happened.
    /// Cranelift-compiled code is expected to test for this failure sentinel
    /// and process it accordingly.
    fn maybe_catch_unwind(f: impl FnOnce() -> Self) -> (Self::Abi, Option<UnwindReason>);
}

// Base case implementations that do not catch unwinds. These are for libcalls
// that neither trap nor execute user code. The raw value is the ABI itself.
//
// Panics in these libcalls will result in a process abort as unwinding is not
// allowed via Rust through `extern "C"` function boundaries.
macro_rules! host_result_no_catch {
    ($($t:ty,)*) => {
        $(
            impl HostResult for $t {
                type Abi = $t;
                fn maybe_catch_unwind(f: impl FnOnce() -> $t) -> ($t, Option<UnwindReason>) {
                    (f(), None)
                }
            }
        )*
    }
}

host_result_no_catch! {
    (),
    bool,
    u32,
    *mut u8,
    u64,
}

impl HostResult for NonNull<u8> {
    type Abi = *mut u8;
    fn maybe_catch_unwind(f: impl FnOnce() -> Self) -> (*mut u8, Option<UnwindReason>) {
        (f().as_ptr(), None)
    }
}

impl<T, E> HostResult for Result<T, E>
where
    T: HostResultHasUnwindSentinel,
    E: Into<TrapReason>,
{
    type Abi = T::Abi;

    fn maybe_catch_unwind(f: impl FnOnce() -> Self) -> (Self::Abi, Option<UnwindReason>) {
        let f = move || match f() {
            Ok(ret) => (ret.into_abi(), None),
            Err(reason) => (T::SENTINEL, Some(UnwindReason::Trap(reason.into()))),
        };

        crate::panic::catch_unwind(AssertUnwindSafe(f))
            .unwrap_or_else(|payload| (T::SENTINEL, Some(UnwindReason::Panic(payload))))
    }
}

/// Trait used in conjunction with `HostResult for Result<T, E>` where this is
/// the trait bound on `T`.
///
/// This is for values in the "ok" position of a `Result` return value. Each
/// value can have a separate ABI from itself (e.g. `type Abi`) and must be
/// convertible to the ABI. Additionally all implementations of this trait have
/// a "sentinel value" which indicates that an unwind happened. This means that
/// no valid instance of `Self` should generate the `SENTINEL` via the
/// `into_abi` function.
pub unsafe trait HostResultHasUnwindSentinel {
    /// The Cranelift-understood ABI of this value (should not be `Self`).
    type Abi: Copy;

    /// A value that indicates that an unwind should happen and is tested for in
    /// Cranelift-generated code.
    const SENTINEL: Self::Abi;

    /// Converts this value into the ABI representation. Should never returned
    /// the `SENTINEL` value.
    fn into_abi(self) -> Self::Abi;
}

/// No return value from the host is represented as a `bool` in the ABI. Here
/// `true` means that execution succeeded while `false` is the sentinel used to
/// indicate an unwind.
unsafe impl HostResultHasUnwindSentinel for () {
    type Abi = bool;
    const SENTINEL: bool = false;
    fn into_abi(self) -> bool {
        true
    }
}

unsafe impl HostResultHasUnwindSentinel for NonZeroU32 {
    type Abi = u32;
    const SENTINEL: Self::Abi = 0;
    fn into_abi(self) -> Self::Abi {
        self.get()
    }
}

/// A 32-bit return value can be inflated to a 64-bit return value in the ABI.
/// In this manner a successful result is a zero-extended 32-bit value and the
/// failure sentinel is `u64::MAX` or -1 as a signed integer.
unsafe impl HostResultHasUnwindSentinel for u32 {
    type Abi = u64;
    const SENTINEL: u64 = u64::MAX;
    fn into_abi(self) -> u64 {
        self.into()
    }
}

/// If there is not actual successful result (e.g. an empty enum) then the ABI
/// can be `()`, or nothing, because there's no successful result and it's
/// always a failure.
unsafe impl HostResultHasUnwindSentinel for core::convert::Infallible {
    type Abi = ();
    const SENTINEL: () = ();
    fn into_abi(self) {
        match self {}
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
        vm_store_context: *const VMStoreContext,
        activation: &Activation,
        trap_pc_and_fp: Option<(VirtualAddress, VirtualAddress)>,
    ) -> Self {
        let mut frames = vec![];
        Self::trace_with_trap_state(vm_store_context, activation, trap_pc_and_fp, |frame| {
            frames.push(frame);
            ControlFlow::Continue(())
        });
        Self(frames)
    }

    /// Walk the current Wasm stack, calling `f` for each frame we walk.
    pub(crate) fn trace_with_trap_state(
        vm_store_context: *const VMStoreContext,
        activation: &Activation,
        trap_pc_and_fp: Option<(VirtualAddress, VirtualAddress)>,
        mut f: impl FnMut(Frame) -> ControlFlow<()>,
    ) {
        unsafe {
            tracing::trace!("====== Capturing Backtrace ======");

            let (last_wasm_exit_pc, last_wasm_exit_fp) = match trap_pc_and_fp {
                // If we exited Wasm by catching a trap, then the Wasm-to-host
                // trampoline did not get a chance to save the last Wasm PC and FP,
                // and we need to use the plumbed-through values instead.
                Some((pc, fp)) => {
                    assert!(core::ptr::eq(
                        vm_store_context,
                        activation.vm_store_context.as_ptr()
                    ));
                    (pc, fp)
                }
                // Either there is no Wasm currently on the stack, or we exited Wasm
                // through the Wasm-to-host trampoline.
                None => {
                    let pc = *(*vm_store_context).last_wasm_exit_pc.get();
                    let fp = *(*vm_store_context).last_wasm_exit_fp.get();
                    (pc, fp)
                }
            };

            let activations = core::iter::once((
                last_wasm_exit_pc,
                last_wasm_exit_fp,
                *(*vm_store_context).last_wasm_entry_fp.get(),
            ))
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

pub fn handle_wasm_exception(
    pc: VirtualAddress,
    fp: VirtualAddress,
    faulting_addr: VirtualAddress,
) -> ControlFlow<()> {
    if let Some(activation) = NonNull::new(ACTIVATION.get()) {
        #[expect(clippy::undocumented_unsafe_blocks, reason = "")]
        let activation = unsafe { activation.as_ref() };

        let Some((code, text_offset)) = lookup_code(pc.get()) else {
            tracing::debug!("no JIT code registered for pc {pc}");
            return ControlFlow::Continue(());
        };

        let Some(trap) = code.lookup_trap_code(text_offset) else {
            tracing::debug!("no JIT trap registered for pc {pc}");
            return ControlFlow::Continue(());
        };

        // record the unwind details
        let backtrace = RawBacktrace::new(
            activation.vm_store_context.as_ptr(),
            activation,
            Some((pc, fp)),
        );
        activation.unwind.set(Some((
            UnwindReason::Trap(TrapReason::Jit {
                pc,
                faulting_addr: Some(faulting_addr),
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
