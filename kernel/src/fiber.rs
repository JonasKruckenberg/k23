use crate::arch;
use crate::vm::{AddressSpace, UserMmap, VirtualAddress};
use alloc::boxed::Box;
use alloc::string::ToString;
use core::arch::naked_asm;
use core::cell::Cell;
use core::marker::PhantomData;
use core::panic::AssertUnwindSafe;
use core::ptr;
use core::ptr::addr_of_mut;
use core::range::Range;

#[derive(Debug)]
pub struct FiberStack(UserMmap);

impl FiberStack {
    pub fn new(aspace: &mut AddressSpace) -> Self {
        let stack_size = 16 * arch::PAGE_SIZE;
        let mmap = UserMmap::new_zeroed(
            aspace,
            stack_size,
            arch::PAGE_SIZE,
            Some("FiberStack".to_string()),
        )
        .unwrap();
        mmap.commit(aspace, Range::from(0..stack_size), true)
            .unwrap();
        Self(mmap)
    }

    pub fn top(&self) -> *mut u8 {
        // Safety: UserMmap guarantees the base pointer and length are valid
        unsafe { self.0.as_ptr().cast_mut().byte_add(self.0.len()) }
    }

    pub fn guard_range(&self) -> Option<Range<*mut u8>> {
        None
    }

    pub fn range(&self) -> Range<VirtualAddress> {
        self.0.range()
    }
}

pub struct Suspend<Resume, Yield, Return> {
    top_of_stack: *mut u8,
    _phantom: PhantomData<(Resume, Yield, Return)>,
}

enum RunResult<Resume, Yield, Return> {
    Executing,
    Resuming(Resume),
    Yield(Yield),
    Returned(Return),
    Panicked(Box<dyn core::any::Any + Send>),
}

pub struct Fiber<'a, Resume, Yield, Return> {
    stack: Option<FiberStack>,
    done: Cell<bool>,
    _phantom: PhantomData<&'a (Resume, Yield, Return)>,
}

impl<'a, Resume, Yield, Return> Fiber<'a, Resume, Yield, Return> {
    /// Creates a new fiber which will execute `func` on the given stack.
    ///
    /// This function returns a `Fiber` which, when resumed, will execute `func`
    /// to completion. When desired the `func` can suspend itself via
    /// `Fiber::suspend`.
    pub fn new<F>(stack: FiberStack, mut f: F) -> Self
    where
        F: FnOnce(Resume, &mut Suspend<Resume, Yield, Return>) -> Return + 'a,
    {
        extern "C" fn fiber_start<F, Resume, Yield, Return>(
            closure_ptr: *mut u8,
            top_of_stack: *mut u8,
        ) where
            F: FnOnce(Resume, &mut Suspend<Resume, Yield, Return>) -> Return,
        {
            let mut suspend = Suspend {
                top_of_stack,
                _phantom: PhantomData,
            };

            // Safety: code below & generics ensure the ptr is a valid `F` ptr
            suspend.execute(unsafe { closure_ptr.cast::<F>().read() });
        }

        let closure_ptr = addr_of_mut!(f);

        // Safety: TODO
        unsafe {
            fiber_init(
                stack.top(),
                fiber_start::<F, Resume, Yield, Return>,
                closure_ptr.cast(),
            );
        }

        Self {
            stack: Some(stack),
            done: Cell::new(false),
            _phantom: PhantomData,
        }
    }

    /// Resumes execution of this fiber.
    ///
    /// This function will transfer execution to the fiber and resume from where
    /// it last left off.
    ///
    /// Returns `true` if the fiber finished or `false` if the fiber was
    /// suspended in the middle of execution.
    ///
    /// # Panics
    ///
    /// Panics if the current thread is already executing a fiber or if this
    /// fiber has already finished.
    ///
    /// Note that if the fiber itself panics during execution then the panic
    /// will be propagated to this caller.
    pub fn resume(&self, val: Resume) -> Result<Return, Yield> {
        assert!(!self.done.replace(true), "cannot resume a finished fiber");
        let result = Cell::new(RunResult::Resuming(val));

        // Safety: TODO
        unsafe {
            debug_assert!(
                self.stack.as_ref().unwrap().top().addr() % 16 == 0,
                "stack needs to be 16-byte aligned"
            );

            // Store where our result is going at the very tip-top of the
            // stack, otherwise known as our reserved slot for this information.
            //
            // In the diagram above this is updating address 0xAff8
            #[expect(clippy::cast_ptr_alignment, reason = "checked above")]
            let addr = self
                .stack
                .as_ref()
                .unwrap()
                .top()
                .cast::<usize>()
                .offset(-1);
            addr.write(ptr::from_ref(&result) as usize);

            fiber_switch(self.stack.as_ref().unwrap().top());

            // null this out to help catch use-after-free
            addr.write(0);
        }

        match result.into_inner() {
            RunResult::Resuming(_) | RunResult::Executing => unreachable!(),
            RunResult::Yield(y) => {
                self.done.set(false);
                Err(y)
            }
            RunResult::Returned(r) => Ok(r),
            RunResult::Panicked(_payload) => {
                crate::panic::begin_unwind(_payload);
            }
        }
    }

    /// Returns whether this fiber has finished executing.
    pub fn done(&self) -> bool {
        self.done.get()
    }

    /// Gets the stack associated with this fiber.
    pub fn stack(&self) -> &FiberStack {
        self.stack.as_ref().unwrap()
    }

    /// When this fiber has finished executing, reclaim its stack.
    pub fn into_stack(mut self) -> FiberStack {
        assert!(self.done());
        self.stack.take().unwrap()
    }
}

impl<Resume, Yield, Return> Suspend<Resume, Yield, Return> {
    /// Suspend execution of a currently running fiber.
    ///
    /// This function will switch control back to the original caller of
    /// `Fiber::resume`. This function will then return once the `Fiber::resume`
    /// function is called again.
    ///
    /// # Panics
    ///
    /// Panics if the current thread is not executing a fiber from this library.
    pub fn suspend(&mut self, value: Yield) -> Resume {
        self.switch(RunResult::Yield(value))
    }

    fn switch(&mut self, result: RunResult<Resume, Yield, Return>) -> Resume {
        // Safety: TODO
        unsafe {
            // Calculate 0xAff8 and then write to it
            (*self.result_location()).set(result);
            fiber_switch(self.top_of_stack);

            self.take_resume()
        }
    }

    unsafe fn take_resume(&self) -> Resume {
        // Safety: TODO
        let prev = unsafe { (*self.result_location()).replace(RunResult::Executing) };
        match prev {
            RunResult::Resuming(val) => val,
            _ => panic!("not in resuming state"),
        }
    }

    unsafe fn result_location(&self) -> *const Cell<RunResult<Resume, Yield, Return>> {
        #[expect(clippy::cast_ptr_alignment, reason = "checked above")]
        // Safety: TODO
        let ret = unsafe { self.top_of_stack.cast::<*const u8>().offset(-1).read() };
        assert!(!ret.is_null());
        ret.cast()
    }

    pub fn execute<F>(&mut self, func: F)
    where
        F: FnOnce(Resume, &mut Suspend<Resume, Yield, Return>) -> Return,
    {
        // Safety: TODO
        let initial = unsafe { self.take_resume() };

        let result = crate::panic::catch_unwind(AssertUnwindSafe(|| (func)(initial, self)));
        self.switch(match result {
            Ok(result) => RunResult::Returned(result),
            Err(panic) => RunResult::Panicked(panic),
        });
    }
}

impl<A, B, C> Drop for Fiber<'_, A, B, C> {
    fn drop(&mut self) {
        debug_assert!(self.done.get(), "fiber dropped without finishing");
    }
}

#[naked]
unsafe extern "C" fn fiber_switch(top_of_stack: *mut u8) {
    // Safety: inline assembly
    unsafe {
        naked_asm! {
            // FIXME this is a workaround for bug in rustc/llvm
            //  https://github.com/rust-lang/rust/issues/80608#issuecomment-1094267279
            ".attribute arch, \"rv64gc\"",

            // We're switching to arbitrary code somewhere else, so pessimistically
            // assume that all callee-save register are clobbered. This means we need
            // to save/restore all of them.
            //
            // Note that this order for saving is important since we use CFI directives
            // below to point to where all the saved registers are.
            "sd   ra,     -0x8(sp)",
            "sd   fp,     -0x10(sp)",
            "sd   s1,     -0x18(sp)",
            "sd   s2,     -0x20(sp)",
            "sd   s3,     -0x28(sp)",
            "sd   s4,     -0x30(sp)",
            "sd   s5,     -0x38(sp)",
            "sd   s6,     -0x40(sp)",
            "sd   s7,     -0x48(sp)",
            "sd   s8,     -0x50(sp)",
            "sd   s9,     -0x58(sp)",
            "sd   s10,    -0x60(sp)",
            "sd   s11,    -0x68(sp)",
            "fsd  fs0,    -0x70(sp)",
            "fsd  fs1,    -0x78(sp)",
            "fsd  fs2,    -0x80(sp)",
            "fsd  fs3,    -0x88(sp)",
            "fsd  fs4,    -0x90(sp)",
            "fsd  fs5,    -0x98(sp)",
            "fsd  fs6,    -0xa0(sp)",
            "fsd  fs7,    -0xa8(sp)",
            "fsd  fs8,    -0xb0(sp)",
            "fsd  fs9,    -0xb8(sp)",
            "fsd  fs10,   -0xc0(sp)",
            "fsd  fs11,   -0xc8(sp)",
            "addi sp, sp, -0xd0",

            "ld   t0,     -0x10(a0)",
            "sd   sp,     -0x10(a0)",

            // Swap stacks and restore all our callee-saved registers
            "mv   sp,     t0",

            "fld  fs11,   0x8(sp)",
            "fld  fs10,   0x10(sp)",
            "fld  fs9,    0x18(sp)",
            "fld  fs8,    0x20(sp)",
            "fld  fs7,    0x28(sp)",
            "fld  fs6,    0x30(sp)",
            "fld  fs5,    0x38(sp)",
            "fld  fs4,    0x40(sp)",
            "fld  fs3,    0x48(sp)",
            "fld  fs2,    0x50(sp)",
            "fld  fs1,    0x58(sp)",
            "fld  fs0,    0x60(sp)",
            "ld   s11,    0x68(sp)",
            "ld   s10,    0x70(sp)",
            "ld   s9,     0x78(sp)",
            "ld   s8,     0x80(sp)",
            "ld   s7,     0x88(sp)",
            "ld   s6,     0x90(sp)",
            "ld   s5,     0x98(sp)",
            "ld   s4,     0xa0(sp)",
            "ld   s3,     0xa8(sp)",
            "ld   s2,     0xb0(sp)",
            "ld   s1,     0xb8(sp)",
            "ld   fp,     0xc0(sp)",
            "ld   ra,     0xc8(sp)",
            "addi sp, sp, 0xd0",
            "jr   ra"
        }
    }
}

#[naked]
unsafe extern "C" fn fiber_init(
    top_of_stack: *mut u8,
    entry: extern "C" fn(*mut u8, *mut u8),
    entry_arg0: *mut u8,
) {
    // Safety: inline assembly
    unsafe {
        naked_asm! {
            "lla t0, {fiber_start}",
            "sd  t0, -0x18(a0)",  // ra,first should be wasmtime_fiber_start.
            "sd  a0, -0x20(a0)",  // fp pointer.
            "sd  a1, -0x28(a0)", // entry_point will load to s1.
            "sd  a2, -0x30(a0)",  // entry_arg0 will load to s2.

            //
            "addi t0, a0,-0xe0",
            "sd   t0, -0x10(a0)",
            "ret",
            fiber_start = sym fiber_start
        }
    }
}

#[naked]
unsafe extern "C" fn fiber_start() {
    // Safety: inline assembly
    unsafe {
        naked_asm! {
            "
            .cfi_startproc
            .cfi_escape 0x0f, /* DW_CFA_def_cfa_expression */ \
            5,             /* the byte length of this expression */ \
            0x52,          /* DW_OP_reg2 (sp) */ \
            0x06,          /* DW_OP_deref */ \
            0x08, 0xd0 ,   /* DW_OP_const1u 0xc8 */ \
            0x22           /* DW_OP_plus */

            .cfi_rel_offset ra,-0x8
            .cfi_rel_offset fp,-0x10
            .cfi_rel_offset s1,-0x18
            .cfi_rel_offset s2,-0x20
            .cfi_rel_offset s3,-0x28
            .cfi_rel_offset s4,-0x30
            .cfi_rel_offset s5,-0x38
            .cfi_rel_offset s6,-0x40
            .cfi_rel_offset s7,-0x48
            .cfi_rel_offset s8,-0x50
            .cfi_rel_offset s9,-0x58
            .cfi_rel_offset s10,-0x60
            .cfi_rel_offset s11,-0x68
            .cfi_rel_offset fs0,-0x70
            .cfi_rel_offset fs1,-0x78
            .cfi_rel_offset fs2,-0x80
            .cfi_rel_offset fs3,-0x88
            .cfi_rel_offset fs4,-0x90
            .cfi_rel_offset fs5,-0x98
            .cfi_rel_offset fs6,-0xa0
            .cfi_rel_offset fs7,-0xa8
            .cfi_rel_offset fs8,-0xb0
            .cfi_rel_offset fs9,-0xb8
            .cfi_rel_offset fs10,-0xc0
            .cfi_rel_offset fs11,-0xc8

            mv a0,s2
            mv a1,fp
            jalr s1
            // .4byte 0 will cause panic.
            // for safety just like x86_64.rs.
            .4byte 0
            .cfi_endproc
            "
        }
    }
}
