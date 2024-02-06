use crate::arch::backtrace as arch;
use crate::arch::backtrace::Context;
use core::ptr::addr_of;
use core::slice;
use gimli::{
    BaseAddresses, CfaRule, EhFrame, EndianSlice, FrameDescriptionEntry, NativeEndian, Register,
    RegisterRule, UnwindContext, UnwindSection, UnwindTableRow,
};

// Load bearing inline don't remove
// TODO figure out why this is and remove
#[inline(always)]
pub fn trace<F: FnMut(&Frame)>(cb: F) {
    let ctx = Context::capture();

    trace_with_context(ctx, cb);
}

// Load bearing inline don't remove
// TODO figure out why this is and remove
#[inline(always)]
pub fn trace_with_context<F: FnMut(&Frame)>(mut ctx: Context, mut cb: F) {
    extern "C" {
        static __eh_frame_start: u8;
        static __eh_frame_end: u8;
    }

    let slice = unsafe {
        let start = addr_of!(__eh_frame_start);

        let end = (start as usize).saturating_add(isize::MAX as _);
        let len = end - start as usize;

        slice::from_raw_parts(start, len)
    };

    let bases = BaseAddresses::default()
        .set_eh_frame(slice.as_ptr() as _)
        .set_text(0x80200000);

    let eh_frame = EhFrame::new(slice, NativeEndian);

    let mut unwinder = UnwindContext::<_, StoreOnStack>::new_in();

    loop {
        let frame = Frame::from_context(&ctx, &eh_frame, &bases, &mut unwinder);

        if let Some(frame) = frame {
            cb(&frame);
            ctx = frame.unwind(&ctx);
        } else {
            return;
        }
    }
}

pub struct Frame<'a> {
    // the program counter
    pc: usize,
    // the stack pointer
    sp: usize,
    fde: FrameDescriptionEntry<EndianSlice<'static, NativeEndian>>,
    row: &'a UnwindTableRow<EndianSlice<'static, NativeEndian>, StoreOnStack>,
}

impl<'a> Frame<'a> {
    fn from_context(
        ctx: &arch::Context,
        eh_frame: &'a EhFrame<EndianSlice<'static, NativeEndian>>,
        bases: &BaseAddresses,
        unwinder: &'a mut UnwindContext<EndianSlice<'static, NativeEndian>, StoreOnStack>,
    ) -> Option<Self> {
        let ra = ctx.return_address();

        if ra == 0 {
            return None;
        }

        let fde = eh_frame
            .fde_for_address(bases, ra as _, EhFrame::cie_from_offset)
            .unwrap();

        let row = fde
            .unwind_info_for_address(eh_frame, bases, unwinder, ra as _)
            .unwrap();

        Some(Frame {
            pc: ra,
            sp: ctx.stack_pointer(),
            fde,
            row,
        })
    }

    pub fn unwind(&self, ctx: &arch::Context) -> arch::Context {
        let row = &self.row;
        let mut new_ctx = ctx.clone();

        let cfa = match *row.cfa() {
            CfaRule::RegisterAndOffset { register, offset } => {
                ctx[register].wrapping_add(offset as usize)
            }
            CfaRule::Expression(_) => panic!("DWARF expressions are unsupported"),
        };

        new_ctx.set_stack_pointer(cfa);
        new_ctx.set_return_address(0);

        for (reg, rule) in row.registers() {
            let value = match *rule {
                RegisterRule::Undefined | RegisterRule::SameValue => ctx[*reg],
                RegisterRule::Offset(offset) => unsafe {
                    *((cfa.wrapping_add(offset as usize)) as *const usize)
                },
                RegisterRule::ValOffset(offset) => cfa.wrapping_add(offset as usize),
                RegisterRule::Register(r) => ctx[r],
                RegisterRule::Expression(_) | RegisterRule::ValExpression(_) => {
                    panic!("DWARF expressions are unsupported")
                }
                RegisterRule::Architectural => unreachable!(),
                RegisterRule::Constant(value) => value as usize,
                _ => unreachable!(),
            };
            new_ctx[*reg] = value;
        }

        new_ctx
    }

    pub fn pc(&self) -> usize {
        self.pc
    }

    pub fn sp(&self) -> usize {
        self.sp
    }

    pub fn symbol_address(&self) -> u64 {
        self.fde.initial_address()
    }
}

struct StoreOnStack;

// gimli's MSRV doesn't allow const generics, so we need to pick a supported array size.
const fn next_value(x: usize) -> usize {
    let supported = [0, 1, 2, 3, 4, 8, 16, 32, 64, 128];
    let mut i = 0;
    while i < supported.len() {
        if supported[i] >= x {
            return supported[i];
        }
        i += 1;
    }
    192
}

const MAX_REG_RULES: usize = 65;

impl<R: gimli::Reader> gimli::UnwindContextStorage<R> for StoreOnStack {
    type Rules = [(Register, RegisterRule<R>); next_value(MAX_REG_RULES)];
    type Stack = [UnwindTableRow<R, Self>; 2];
}
