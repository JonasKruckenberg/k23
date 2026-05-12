// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::{fmt, ops};

use gimli::{Register, RegisterRule, X86_64};

/// The largest register number on this architecture.
pub const MAX_REG: u16 = 65;
// Match DWARF_FRAME_REGISTERS in libgcc
pub const MAX_REG_RULES: usize = 17;

pub const SP: Register = X86_64::RSP;
pub const RA: Register = X86_64::RA;

pub const UNWIND_DATA_REG: (Register, Register) = (X86_64::RAX, X86_64::RDX);

/// Returns the default register rule for the given register on this architecture.
pub fn default_register_rule_for(_reg: Register) -> RegisterRule<usize> {
    // As far as I can tell x86_64 has no special requirements
    RegisterRule::Undefined
}

#[repr(C)]
#[derive(Clone, Default)]
pub struct Registers {
    // Regular registers DWARF used by CFI rules.
    // This includes only registers used in practice by LLVM.
    // source: https://github.com/llvm/llvm-project/blob/de696eeb7051c0d9e4729a6f4a84fc99bb38e904/libunwind/src/Registers.hpp#L373-L383
    pub gp: [usize; 16],
    pub ra: usize,
    // SysV §6.2.1 says the control bits of MXCSR and FCW are caller-saved-ish
    // only the lower 32 bits are preserved
    pub mxcsr: usize,
    // only the lower 16 bits are preserved
    pub fcw: usize,
}

impl fmt::Debug for Registers {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut fmt = fmt.debug_struct("Registers");
        for i in 0..=15 {
            fmt.field(
                X86_64::register_name(Register(i)).unwrap(),
                &self.gp[i as usize],
            );
        }
        fmt.field("ra", &self.ra);
        fmt.field("mxcsr", &self.mxcsr);
        fmt.field("fcw", &self.fcw);
        fmt.finish()
    }
}

impl ops::Index<Register> for Registers {
    type Output = usize;

    fn index(&self, reg: Register) -> &usize {
        match reg {
            Register(0..=15) => &self.gp[reg.0 as usize],
            X86_64::RA => &self.ra,
            X86_64::MXCSR => &self.mxcsr,
            X86_64::FCW => &self.fcw,
            _ => unimplemented!("register {reg:?}"),
        }
    }
}

impl ops::IndexMut<Register> for Registers {
    fn index_mut(&mut self, reg: Register) -> &mut usize {
        match reg {
            Register(0..=15) => &mut self.gp[reg.0 as usize],
            X86_64::RA => &mut self.ra,
            X86_64::MXCSR => &mut self.mxcsr,
            X86_64::FCW => &mut self.fcw,
            _ => unimplemented!("register {reg:?}"),
        }
    }
}

#[unsafe(naked)]
pub extern "C-unwind" fn save_context(f: extern "C" fn(&mut Registers, *mut ()), ptr: *mut ()) {
    // No need to save caller-saved registers here.
    core::arch::naked_asm!(
        ".cfi_startproc",
        "sub rsp, 0x98",
        ".cfi_def_cfa_offset 0xA0",
        "
                mov [rsp + 0x18], rbx
                mov [rsp + 0x30], rbp

                /* Adjust the stack to account for the return address */
                lea rax, [rsp + 0xA0]
                mov [rsp + 0x38], rax

                mov [rsp + 0x60], r12
                mov [rsp + 0x68], r13
                mov [rsp + 0x70], r14
                mov [rsp + 0x78], r15

                /* Return address */
                mov rax, [rsp + 0x98]
                mov [rsp + 0x80], rax

                stmxcsr [rsp + 0x88]
                fnstcw [rsp + 0x90]

                mov rax, rdi
                mov rdi, rsp
                call rax
                add rsp, 0x98
                ",
        ".cfi_def_cfa_offset 8",
        "ret",
        ".cfi_endproc",
    );
}

/// # Safety
///
/// This function will restore whatever values are in the given `Context` into the machine registers
/// **without** performing any sort of validation. The caller must ensure at least:
/// 1. `RSP` `regs.gp[7]` is a valid, correctly-aligned, writable stack address.
/// 2. `RA` `regs.ra` is a valid, correctly-aligned, code pointer.
pub unsafe fn restore_context(regs: &Registers) -> ! {
    unsafe {
        core::arch::asm!(
            "
                /* Restore stack */
                mov rsp, [rdi + 0x38]

                /* Restore callee-saved control registers */
                ldmxcsr [rdi + 0x88]
                fldcw [rdi + 0x90]

                /* Restore return address */
                mov rax, [rdi + 0x80]
                push rax

                /*
                * Restore general-purpose registers. Non-callee-saved registers are
                * also restored because sometimes it's used to pass unwind arguments.
                */
                mov rax, [rdi + 0x00]
                mov rdx, [rdi + 0x08]
                mov rcx, [rdi + 0x10]
                mov rbx, [rdi + 0x18]
                mov rsi, [rdi + 0x20]
                mov rbp, [rdi + 0x30]
                mov r8 , [rdi + 0x40]
                mov r9 , [rdi + 0x48]
                mov r10, [rdi + 0x50]
                mov r11, [rdi + 0x58]
                mov r12, [rdi + 0x60]
                mov r13, [rdi + 0x68]
                mov r14, [rdi + 0x70]
                mov r15, [rdi + 0x78]

                /* RDI restored last */
                mov rdi, [rdi + 0x28]

                ret
                ",
            in("rdi") regs,
            options(noreturn)
        );
    }
}
