macro_rules! csr_base_and_read {
    ($ty_name: ident, $csr_name: literal) => {
        pub fn read() -> $ty_name {
            cfg_if::cfg_if! {
                if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                    let bits: usize;
                    // force $csrname to be a string literal
                    let csr_name: &str = $csr_name;
                    unsafe {
                        ::core::arch::asm!(concat!("csrr {0}, ", $csr_name), out(reg) bits);
                    }

                    $ty_name { bits }
                } else {
                    unimplemented!()
                }
            }
        }

        pub struct $ty_name {
            bits: usize,
        }

        impl $ty_name {
            /// Returns the contents of the register as raw bits
            #[inline]
            pub fn as_bits(&self) -> usize {
                self.bits
            }
        }
    };
}

macro_rules! csr_write {
    ($csr_name: literal) => {
        /// Writes the CSR
        #[inline]
        #[allow(unused_variables)]
        unsafe fn _write(bits: usize) {
            cfg_if::cfg_if! {
                if #[cfg(any(target_arch = "riscv64", target_arch = "riscv32"))] {
                    let csr_name: &str = $csr_name;
                    ::core::arch::asm!(concat!("csrrw x0, ", $csr_name, ", {0}"), in(reg) bits)
                } else {
                    unimplemented!()
                }
            }
        }
    };
}

pub mod scause {
    use crate::arch::riscv64::register::stvec::Mode;
    csr_base_and_read!(Scause, "scause");
    csr_write!("scause");

    pub unsafe fn write(trap: Trap) {
        match trap {
            Trap::Interrupt(interrupt) => {
                _write(1 << (usize::BITS as usize - 1) | interrupt as usize)
            }
            Trap::Exception(exception) => _write(exception as usize),
        }
    }

    impl Scause {
        /// Returns the code field
        #[inline]
        pub fn code(&self) -> usize {
            self.bits & !(1 << (usize::BITS as usize - 1))
        }

        /// Is trap cause an interrupt.
        #[inline]
        pub fn is_interrupt(&self) -> bool {
            self.bits & (1 << (usize::BITS as usize - 1)) != 0
        }

        /// Is trap cause an exception.
        #[inline]
        pub fn is_exception(&self) -> bool {
            !self.is_interrupt()
        }

        #[inline]
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
}

pub mod stvec {
    csr_base_and_read!(Stvec, "stvec");
    csr_write!("stvec");

    pub unsafe fn write(base: usize, mode: Mode) {
        _write(base + mode as usize)
    }

    impl Stvec {
        pub fn mode(&self) -> Mode {
            let mode = self.bits & 0b11;
            match mode {
                0 => Mode::Direct,
                1 => Mode::Vectored,
                _ => panic!("unknown trap mode"),
            }
        }
        pub fn base(&self) -> usize {
            self.bits - (self.bits & 0b11)
        }
    }

    pub enum Mode {
        /// All exceptions set `pc` to `BASE`.
        Direct = 0,
        /// Asynchronous interrupts set `pc` to `BASE+4×cause`.
        Vectored = 1,
    }
}

pub mod sepc {
    use core::fmt;
    use core::fmt::Formatter;
    csr_base_and_read!(Sepc, "stvec");

    impl fmt::Debug for Sepc {
        fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
            f.debug_tuple("Sepc").field(&self.bits).finish()
        }
    }
}

pub mod stval {
    use core::fmt;
    use core::fmt::Formatter;
    csr_base_and_read!(Stval, "stvec");

    impl fmt::Debug for Stval {
        fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
            f.debug_tuple("Stval").field(&self.bits).finish()
        }
    }
}
