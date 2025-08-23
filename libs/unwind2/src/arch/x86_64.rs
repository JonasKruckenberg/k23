// Copyright 2025 bubblepipe
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! x86_64 specific unwinding code, mostly saving and restoring registers.
use core::arch::asm;
use core::{fmt, ops};

use gimli::{Register, X86_64};

// Match DWARF_FRAME_REGISTERS in libgcc
pub const MAX_REG_RULES: usize = 17;

/// The largest register number on this architecture.
pub const MAX_REG: u16 = 16;

pub const SP: Register = X86_64::RSP;
pub const RA: Register = X86_64::RA;

pub const UNWIND_DATA_REG: (Register, Register) = (X86_64::RDI, X86_64::RSI);

/// Register context when unwinding.
///
/// This type is architecture-dependent, but generally holds a copy of all registers that are required
/// to look up values while unwinding the stack.
#[repr(C)]
#[derive(Clone, Default)]
pub struct Registers {
    /// General purpose registers
    pub gp: [usize; 17],
}

impl fmt::Debug for Registers {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut fmt = fmt.debug_struct("Context");
        for i in 0..=16u16 {
            fmt.field(
                X86_64::register_name(Register(i)).unwrap_or("unknown"),
                &self.gp[i as usize],
            );
        }
        fmt.finish()
    }
}

impl ops::Index<Register> for Registers {
    type Output = usize;

    fn index(&self, reg: Register) -> &usize {
        match reg {
            Register(0..=16) => &self.gp[reg.0 as usize],
            _ => unimplemented!("register {reg:?}"),
        }
    }
}

impl ops::IndexMut<Register> for Registers {
    fn index_mut(&mut self, reg: Register) -> &mut usize {
        match reg {
            Register(0..=16) => &mut self.gp[reg.0 as usize],
            _ => unimplemented!(),
        }
    }
}

pub extern "C" fn save_context(f: extern "C" fn(&mut Registers, *mut ()), ptr: *mut ()) {
    // Dummy implementation - just create empty registers and call the function
    let mut regs = Registers::default();
    f(&mut regs, ptr);
}

pub extern "C" fn restore_context(_regs: &Registers) -> ! {
    // Dummy implementation - just loop forever
    loop {
        unsafe {
            asm!("pause");
        }
    }
}
