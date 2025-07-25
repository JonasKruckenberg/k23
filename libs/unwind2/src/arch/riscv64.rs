// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! RISC-V specific unwinding code, mostly saving and restoring registers.
use core::arch::{asm, naked_asm};
use core::{fmt, ops};

use cfg_if::cfg_if;
use gimli::{Register, RiscV};

// Match DWARF_FRAME_REGISTERS in libgcc
pub const MAX_REG_RULES: usize = 65;

cfg_if! {
    if #[cfg(target_feature = "d")] {
        /// The largest register number on this architecture.
        pub const MAX_REG: u16 = 64;
    } else {
        /// The largest register number on this architecture.
        pub const MAX_REG: u16 = 32;
    }
}

pub const SP: Register = RiscV::SP;
pub const RA: Register = RiscV::RA;

pub const UNWIND_DATA_REG: (Register, Register) = (RiscV::A0, RiscV::A1);

#[cfg(all(target_feature = "f", not(target_feature = "d")))]
compile_error!("RISC-V with only F extension is not supported");

/// Register context when unwinding.
///
/// This type is architecture-dependent, but generally holds a copy of all registers that are required
/// to look up values while unwinding the stack.
#[repr(C)]
#[derive(Clone, Default)]
pub struct Registers {
    /// General purpose registers
    pub gp: [usize; 32],
    #[cfg(target_feature = "d")]
    /// Floating point registers
    pub fp: [usize; 32],
}

impl fmt::Debug for Registers {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut fmt = fmt.debug_struct("Context");
        for i in 0..=31u16 {
            fmt.field(
                RiscV::register_name(Register(i)).unwrap(),
                &self.gp[i as usize],
            );
        }
        #[cfg(target_feature = "d")]
        for i in 0..=31u16 {
            fmt.field(
                RiscV::register_name(Register(i + 32)).unwrap(),
                &self.fp[i as usize],
            );
        }
        fmt.finish()
    }
}

impl ops::Index<Register> for Registers {
    type Output = usize;

    fn index(&self, reg: Register) -> &usize {
        match reg {
            Register(0..=31) => &self.gp[reg.0 as usize],
            #[cfg(target_feature = "d")]
            Register(32..=63) => &self.fp[(reg.0 - 32) as usize],
            _ => unimplemented!("register {reg:?}"),
        }
    }
}

impl ops::IndexMut<Register> for Registers {
    fn index_mut(&mut self, reg: Register) -> &mut usize {
        match reg {
            Register(0..=31) => &mut self.gp[reg.0 as usize],
            #[cfg(target_feature = "d")]
            Register(32..=63) => &mut self.fp[(reg.0 - 32) as usize],
            _ => unimplemented!(),
        }
    }
}

macro_rules! code {
    (save_gp) => {
        "
        sd x0, 0x00(sp)
        sd ra, 0x08(sp)
        sd t0, 0x10(sp)
        sd gp, 0x18(sp)
        sd tp, 0x20(sp)
        sd s0, 0x40(sp)
        sd s1, 0x48(sp)
        sd s2, 0x90(sp)
        sd s3, 0x98(sp)
        sd s4, 0xA0(sp)
        sd s5, 0xA8(sp)
        sd s6, 0xB0(sp)
        sd s7, 0xB8(sp)
        sd s8, 0xC0(sp)
        sd s9, 0xC8(sp)
        sd s10, 0xD0(sp)
        sd s11, 0xD8(sp)
        "
    };
    (save_fp) => {
        "
        fsd fs0, 0x140(sp)
        fsd fs1, 0x148(sp)
        fsd fs2, 0x190(sp)
        fsd fs3, 0x198(sp)
        fsd fs4, 0x1A0(sp)
        fsd fs5, 0x1A8(sp)
        fsd fs6, 0x1B0(sp)
        fsd fs7, 0x1B8(sp)
        fsd fs8, 0x1C0(sp)
        fsd fs9, 0x1C8(sp)
        fsd fs10, 0x1D0(sp)
        fsd fs11, 0x1D8(sp)
        "
    };
    (restore_gp) => {
        "
        ld ra, 0x08(a0)
        ld sp, 0x10(a0)
        ld gp, 0x18(a0)
        ld tp, 0x20(a0)
        ld t0, 0x28(a0)
        ld t1, 0x30(a0)
        ld t2, 0x38(a0)
        ld s0, 0x40(a0)
        ld s1, 0x48(a0)
        ld a1, 0x58(a0)
        ld a2, 0x60(a0)
        ld a3, 0x68(a0)
        ld a4, 0x70(a0)
        ld a5, 0x78(a0)
        ld a6, 0x80(a0)
        ld a7, 0x88(a0)
        ld s2, 0x90(a0)
        ld s3, 0x98(a0)
        ld s4, 0xA0(a0)
        ld s5, 0xA8(a0)
        ld s6, 0xB0(a0)
        ld s7, 0xB8(a0)
        ld s8, 0xC0(a0)
        ld s9, 0xC8(a0)
        ld s10, 0xD0(a0)
        ld s11, 0xD8(a0)
        ld t3, 0xE0(a0)
        ld t4, 0xE8(a0)
        ld t5, 0xF0(a0)
        ld t6, 0xF8(a0)
        "
    };
    (restore_fp) => {
        "
        fld ft0, 0x100(a0)
        fld ft1, 0x108(a0)
        fld ft2, 0x110(a0)
        fld ft3, 0x118(a0)
        fld ft4, 0x120(a0)
        fld ft5, 0x128(a0)
        fld ft6, 0x130(a0)
        fld ft7, 0x138(a0)
        fld fs0, 0x140(a0)
        fld fs1, 0x148(a0)
        fld fa0, 0x150(a0)
        fld fa1, 0x158(a0)
        fld fa2, 0x160(a0)
        fld fa3, 0x168(a0)
        fld fa4, 0x170(a0)
        fld fa5, 0x178(a0)
        fld fa6, 0x180(a0)
        fld fa7, 0x188(a0)
        fld fs2, 0x190(a0)
        fld fs3, 0x198(a0)
        fld fs4, 0x1A0(a0)
        fld fs5, 0x1A8(a0)
        fld fs6, 0x1B0(a0)
        fld fs7, 0x1B8(a0)
        fld fs8, 0x1C0(a0)
        fld fs9, 0x1C8(a0)
        fld fs10, 0x1D0(a0)
        fld fs11, 0x1D8(a0)
        fld ft8, 0x1E0(a0)
        fld ft9, 0x1E8(a0)
        fld ft10, 0x1F0(a0)
        fld ft11, 0x1F8(a0)
        "
    };
}

#[unsafe(naked)]
pub extern "C-unwind" fn save_context(f: extern "C" fn(&mut Registers, *mut ()), ptr: *mut ()) {
    cfg_if! {
        if #[cfg(target_feature = "d")] {
            // No need to save caller-saved registers here.
            naked_asm! {
                // FIXME this is a workaround for bug in rustc/llvm
                //  https://github.com/rust-lang/rust/issues/80608#issuecomment-1094267279
                ".attribute arch, \"rv64gc\"",
                "
                    .cfi_startproc
                    mv t0, sp
                    add sp, sp, -0x210
                    .cfi_def_cfa_offset 0x210
                    sd ra, 0x200(sp)
                    .cfi_offset ra, -16
                    ",
                code!(save_gp),
                code!(save_fp),
                "
                    mv t0, a0
                    mv a0, sp
                    jalr t0
                    ld ra, 0x200(sp)
                    add sp, sp, 0x210
                    .cfi_def_cfa_offset 0
                    .cfi_restore ra
                    ret
                    .cfi_endproc
                    "
            };
        } else {
            // No need to save caller-saved registers here.
            naked_asm! {
                "
                    mv t0, sp
                    add sp, sp, -0x110
                    .cfi_def_cfa_offset 0x110
                    sd ra, 0x100(sp)
                    .cfi_offset ra, -16
                    ",
                code!(save_gp),
                "
                    mv t0, a0
                    mv a0, sp
                    jalr t0
                    ld ra, 0x100(sp)
                    add sp, sp, 0x110
                    .cfi_def_cfa_offset 0
                    .cfi_restore ra
                    ret
                    ",
            };
        }
    }
}

/// # Safety
///
/// This function will restore whatever values are in the given `Context` into the machine registers
/// **without** performing any sort of validation.
pub unsafe fn restore_context(ctx: &Registers) -> ! {
    // Safety: inline assembly
    unsafe {
        cfg_if! {
            if #[cfg(target_feature = "d")] {
                asm!(
                    code!(restore_fp),
                    code!(restore_gp),
                    "
                    ld a0, 0x50(a0)
                    ret
                    ",
                    in("a0") ctx,
                    options(noreturn)
                );
            } else {
                asm!(
                    code!(restore_gp),
                    "
                    ld a0, 0x50(a0)
                    ret
                    ",
                    in("a0") ctx,
                    options(noreturn)
                );
            }
        }
    }
}
