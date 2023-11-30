use core::arch::asm;
use core::ops;
use gimli::{Register, RiscV};

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

    pub fn return_address(&self) -> usize {
        self.ra
    }

    pub fn set_return_address(&mut self, ra: usize) {
        self.ra = ra;
    }

    pub fn stack_pointer(&self) -> usize {
        self.sp
    }

    pub fn set_stack_pointer(&mut self, sp: usize) {
        self.sp = sp;
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
