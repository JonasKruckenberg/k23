use core::arch::asm;
use core::ptr::addr_of;
use core::{ops, slice};
use gimli::{
    BaseAddresses, CfaRule, EhFrame, EndianSlice, FrameDescriptionEntry, NativeEndian, Register,
    RegisterRule, RiscV, UnwindContext, UnwindSection, UnwindTableRow,
};

// Load bearing inline don't remove
// TODO figure out why this is and remove
#[inline(always)]
pub fn trace<F: FnMut(&Frame)>(mut cb: F) {
    let mut ctx = Context::capture();

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

// The LLVM source (https://llvm.org/doxygen/RISCVFrameLowering_8cpp_source.html)
// specify that only ra (x1) and saved registers (x8-x9, x18-x27) are used for
// frame unwinding info, plus sp (x2) for the CFA, so we only need to save those.
// If this causes issues down the line it should be trivial to change this to capture the full context.
#[derive(Debug, Clone)]
pub struct Context {
    pub ra: usize,
    pub sp: usize,
    pub s: [usize; 12],
}

impl Context {
    // Load bearing inline don't remove
    // TODO figure out why this is and remove
    #[inline(always)]
    pub fn capture() -> Self {
        let (ra, sp, s0, s1, s2, s3, s4, s5, s6, s7, s8, s9, s10, s11);
        unsafe {
            asm!(
            "mv {}, ra",
            "mv {}, sp",
            "mv {}, s0",
            "mv {}, s1",
            "mv {}, s2",
            "mv {}, s3",
            "mv {}, s4",
            "mv {}, s5",
            "mv {}, s6",
            "mv {}, s7",
            "mv {}, s8",
            "mv {}, s9",
            "mv {}, s10",
            "mv {}, s11",
            out(reg) ra,
            out(reg) sp,
            out(reg) s0,
            out(reg) s1,
            out(reg) s2,
            out(reg) s3,
            out(reg) s4,
            out(reg) s5,
            out(reg) s6,
            out(reg) s7,
            out(reg) s8,
            out(reg) s9,
            out(reg) s10,
            out(reg) s11,
            )
        }

        Self {
            ra,
            sp,
            s: [s0, s1, s2, s3, s4, s5, s6, s7, s8, s9, s10, s11],
        }
    }
}

impl ops::Index<Register> for Context {
    type Output = usize;

    fn index(&self, index: Register) -> &Self::Output {
        match index {
            RiscV::RA => &self.ra,
            RiscV::SP => &self.sp,
            Register(reg @ 8..=9) => &self.s[reg as usize - 8],
            Register(reg @ 18..=27) => &self.s[reg as usize - 16],
            reg => panic!("unsupported register {reg:?}"),
        }
    }
}

impl ops::IndexMut<Register> for Context {
    fn index_mut(&mut self, index: Register) -> &mut Self::Output {
        match index {
            RiscV::RA => &mut self.ra,
            RiscV::SP => &mut self.sp,
            Register(reg @ 8..=9) => &mut self.s[reg as usize - 8],
            Register(reg @ 18..=27) => &mut self.s[reg as usize - 16],
            reg => panic!("unsupported register {reg:?}"),
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
        ctx: &Context,
        eh_frame: &'a EhFrame<EndianSlice<'static, NativeEndian>>,
        bases: &BaseAddresses,
        unwinder: &'a mut UnwindContext<EndianSlice<'static, NativeEndian>, StoreOnStack>,
    ) -> Option<Self> {
        let ra = ctx[RiscV::RA];

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
            sp: ctx[RiscV::SP],
            fde,
            row,
        })
    }

    pub fn unwind(&self, ctx: &Context) -> Context {
        let row = &self.row;
        let mut new_ctx = ctx.clone();

        let cfa = match *row.cfa() {
            CfaRule::RegisterAndOffset { register, offset } => {
                ctx[register].wrapping_add(offset as usize)
            }
            CfaRule::Expression(_) => panic!("DWARF expressions are unsupported"),
        };

        new_ctx[RiscV::SP] = cfa as _;
        new_ctx[RiscV::RA] = 0;

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
