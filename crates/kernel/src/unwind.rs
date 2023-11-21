use core::arch::asm;
use core::ptr::{addr_of, addr_of_mut};
use core::{ops, slice};
use gimli::{
    BaseAddresses, CfaRule, EhFrame, EndianSlice, NativeEndian, Register, RegisterRule, RiscV,
    UnwindContext, UnwindSection, UnwindTableRow,
};

pub struct Backtrace {
    bases: BaseAddresses,
    eh_frame: EhFrame<EndianSlice<'static, NativeEndian>>,
    ctx: Context,
    unwinder: UnwindContext<EndianSlice<'static, NativeEndian>, StoreOnStack>,
}

impl Backtrace {
    #[inline(always)]
    pub fn new() -> Self {
        Self::from_context(Context::capture())
    }

    pub fn from_context(ctx: Context) -> Self {
        extern "C" {
            static __eh_frame_start: u8;
            static __eh_frame_end: u8;
        }

        let eh_frame = unsafe {
            let start = addr_of!(__eh_frame_start);
            let end = addr_of!(__eh_frame_end);

            slice::from_raw_parts(start, end as usize - start as usize)
        };

        let bases = BaseAddresses::default()
            .set_eh_frame(eh_frame.as_ptr() as _)
            .set_text(0x80200000);

        let eh_frame = EhFrame::new(eh_frame, NativeEndian);

        let unwinder = UnwindContext::<_, StoreOnStack>::new_in();

        Self {
            bases,
            eh_frame,
            ctx,
            unwinder,
        }
    }

    fn construct_frame(&mut self, pc: usize) -> Option<Frame> {
        let ra = self.ctx[RiscV::RA];

        if ra == 0 {
            return None;
        }

        let fde = self
            .eh_frame
            .fde_for_address(&self.bases, pc as _, EhFrame::cie_from_offset)
            .unwrap();

        let row = fde
            .unwind_info_for_address(&self.eh_frame, &self.bases, &mut self.unwinder, ra as _)
            .unwrap();

        Some(Frame {
            row: row.clone(),
            pc: ra,
        })
    }

    fn update_regs_from_frame(&mut self, frame: &Frame) {
        let row = &frame.row;

        let cfa = match *row.cfa() {
            CfaRule::RegisterAndOffset { register, offset } => {
                self.ctx[register].wrapping_add(offset as usize)
            }
            CfaRule::Expression(_) => panic!("DWARF expressions are unsupported"),
        };

        self.ctx[RiscV::SP] = cfa as _;
        self.ctx[RiscV::RA] = 0;

        for (reg, rule) in row.registers() {
            let value = match *rule {
                RegisterRule::Undefined | RegisterRule::SameValue => self.ctx[*reg],
                RegisterRule::Offset(offset) => unsafe {
                    *((cfa.wrapping_add(offset as usize)) as *const usize)
                },
                RegisterRule::ValOffset(offset) => cfa.wrapping_add(offset as usize),
                RegisterRule::Register(r) => self.ctx[r],
                RegisterRule::Expression(_) | RegisterRule::ValExpression(_) => {
                    panic!("DWARF expressions are unsupported")
                }
                RegisterRule::Architectural => unreachable!(),
                RegisterRule::Constant(value) => value as usize,
                _ => unreachable!(),
            };
            self.ctx[*reg] = value;
        }
    }
}

impl Iterator for Backtrace {
    type Item = Frame;

    fn next(&mut self) -> Option<Self::Item> {
        let frame = self.construct_frame(self.ctx.ra)?;
        self.update_regs_from_frame(&frame);

        Some(frame)
    }
}

// The LLVM source (https://llvm.org/doxygen/RISCVFrameLowering_8cpp_source.html)
// specify that only ra (x1) and saved registers (x8-x9, x18-x27) are used for
// frame unwinding info, plus sp (x2) for the CFA, so we only need to save those.
// If this causes issues down the line it should be trivial to change this to capture the full context.
#[repr(C)]
#[derive(Clone, Default, Debug)]
pub struct Context {
    pub ra: usize,
    pub sp: usize,
    pub s: [usize; 12],
}

#[cfg(target_pointer_width = "64")]
macro_rules! save_gp {
    ($reg:ident => $ptr:ident[$pos:expr]) => {
        concat!(
            "sd ",
            stringify!($reg),
            ", 8*",
            $pos,
            '(',
            stringify!($ptr),
            ')'
        )
    };
}

impl Context {
    #[naked]
    pub extern "C" fn capture() -> Self {
        unsafe {
            asm!(
                save_gp!(ra => a0[0]),
                save_gp!(sp => a0[1]),
                save_gp!(s0 => a0[2]),
                save_gp!(s1 => a0[3]),
                save_gp!(s2 => a0[4]),
                save_gp!(s3 => a0[5]),
                save_gp!(s4 => a0[6]),
                save_gp!(s5 => a0[7]),
                save_gp!(s6 => a0[8]),
                save_gp!(s7 => a0[9]),
                save_gp!(s8 => a0[10]),
                save_gp!(s9 => a0[11]),
                save_gp!(s10 => a0[12]),
                save_gp!(s11 => a0[13]),
                "ret",
                options(noreturn)
            )
        }
    }
}

pub struct Frame {
    pub pc: usize,
    row: UnwindTableRow<EndianSlice<'static, NativeEndian>, StoreOnStack>,
}

impl ops::Index<Register> for Context {
    type Output = usize;

    fn index(&self, index: Register) -> &Self::Output {
        log::debug!("index {index:?}");
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
        log::debug!("indexMut {index:?}");
        match index {
            RiscV::RA => &mut self.ra,
            RiscV::SP => &mut self.sp,
            Register(reg @ 8..=9) => &mut self.s[reg as usize - 8],
            Register(reg @ 18..=27) => &mut self.s[reg as usize - 16],
            reg => panic!("unsupported register {reg:?}"),
        }
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
