// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Supervisor Cause Register

use super::{read_csr_as, set};
use core::fmt;
use core::fmt::Formatter;

/// scause register
#[derive(Clone, Copy)]
pub struct Scause {
    bits: usize,
}

read_csr_as!(Scause, 0x142);
set!(0x142);

pub unsafe fn set(trap: Trap) {
    match trap {
        Trap::Interrupt(interrupt) => unsafe {
            _set(1 << (usize::BITS as usize - 1) | interrupt as usize)
        },
        Trap::Exception(exception) => unsafe { _set(exception as usize) },
    }
}

impl Scause {
    /// Returns the code field
    #[inline]
    #[must_use]
    pub fn code(&self) -> usize {
        self.bits & !(1 << (usize::BITS as usize - 1))
    }

    /// Is trap cause an interrupt.
    #[inline]
    #[must_use]
    pub fn is_interrupt(&self) -> bool {
        self.bits & (1 << (usize::BITS as usize - 1)) != 0
    }

    /// Is trap cause an exception.
    #[inline]
    #[must_use]
    pub fn is_exception(&self) -> bool {
        !self.is_interrupt()
    }

    /// Returns the cause of the trap.
    ///
    /// # Panics
    ///
    /// Panics if the cause is unknown or invalid.
    #[inline]
    #[must_use]
    pub fn cause(&self) -> Trap {
        if self.is_interrupt() {
            Trap::Interrupt(Interrupt::try_from(self.code()).expect("unknown interrupt"))
        } else {
            Trap::Exception(Exception::try_from(self.code()).expect("unknown exception"))
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Trap {
    Interrupt(Interrupt),
    Exception(Exception),
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Interrupt {
    SupervisorSoft = 1,
    VirtualSupervisorSoft = 2,
    SupervisorTimer = 5,
    VirtualSupervisorTimer = 6,
    SupervisorExternal = 9,
    VirtualSupervisorExternal = 10,
    SupervisorGuestExternal = 12,
}

impl TryFrom<usize> for Interrupt {
    type Error = ();

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::SupervisorSoft),
            2 => Ok(Self::VirtualSupervisorSoft),
            5 => Ok(Self::SupervisorTimer),
            6 => Ok(Self::VirtualSupervisorTimer),
            9 => Ok(Self::SupervisorExternal),
            10 => Ok(Self::VirtualSupervisorExternal),
            12 => Ok(Self::SupervisorGuestExternal),
            _ => Err(()),
        }
    }
}

impl From<Interrupt> for usize {
    fn from(value: Interrupt) -> Self {
        match value {
            Interrupt::SupervisorSoft => 1,
            Interrupt::VirtualSupervisorSoft => 2,
            Interrupt::SupervisorTimer => 5,
            Interrupt::VirtualSupervisorTimer => 6,
            Interrupt::SupervisorExternal => 9,
            Interrupt::VirtualSupervisorExternal => 10,
            Interrupt::SupervisorGuestExternal => 12,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Exception {
    InstructionMisaligned = 0,
    InstructionFault = 1,
    IllegalInstruction = 2,
    Breakpoint = 3,
    LoadMisaligned = 4,
    LoadFault = 5,
    StoreMisaligned = 6,
    StoreFault = 7,
    UserEnvCall = 8,
    SupervisorEnvCall = 9,
    VirtualSupervisorEnvCall = 10,
    InstructionPageFault = 12,
    LoadPageFault = 13,
    StorePageFault = 15,
    InstructionGuestPageFault = 20,
    LoadGuestPageFault = 21,
    VirtualInstruction = 22,
    StoreGuestPageFault = 23,
}

impl TryFrom<usize> for Exception {
    type Error = ();

    #[inline]
    fn try_from(nr: usize) -> Result<Self, Self::Error> {
        match nr {
            0 => Ok(Self::InstructionMisaligned),
            1 => Ok(Self::InstructionFault),
            2 => Ok(Self::IllegalInstruction),
            3 => Ok(Self::Breakpoint),
            4 => Ok(Self::LoadMisaligned),
            5 => Ok(Self::LoadFault),
            6 => Ok(Self::StoreMisaligned),
            7 => Ok(Self::StoreFault),
            8 => Ok(Self::UserEnvCall),
            9 => Ok(Self::SupervisorEnvCall),
            10 => Ok(Self::VirtualSupervisorEnvCall),
            12 => Ok(Self::InstructionPageFault),
            13 => Ok(Self::LoadPageFault),
            15 => Ok(Self::StorePageFault),
            20 => Ok(Self::InstructionGuestPageFault),
            21 => Ok(Self::LoadGuestPageFault),
            22 => Ok(Self::VirtualInstruction),
            23 => Ok(Self::StoreGuestPageFault),
            _ => Err(()),
        }
    }
}

impl fmt::Debug for Scause {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Scause").field(&self.cause()).finish()
    }
}
