// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch;
use crate::eh_info::obtain_eh_info;
use crate::utils::{deref_pointer, get_unlimited_slice, with_context, StoreOnStack};
use fallible_iterator::FallibleIterator;
use gimli::{
    CfaRule, EhFrame, EndianSlice, FrameDescriptionEntry, NativeEndian, Register, RegisterRule,
    UnwindSection, UnwindTableRow,
};

#[derive(Debug)]
pub struct Frame<'a> {
    regs: arch::Registers,
    fde: FrameDescriptionEntry<EndianSlice<'a, NativeEndian>, usize>,
    row: UnwindTableRow<usize, StoreOnStack>,
    return_address_register: Register,
}

impl<'a> Frame<'a> {
    pub fn ip(&self) -> usize {
        self.regs[self.return_address_register]
    }

    pub fn sp(&self) -> usize {
        self.regs[arch::SP]
    }

    pub fn personality(&self) -> Option<u64> {
        // Safety: we have to trust the DWARF info here
        self.fde.personality().map(|x| unsafe { deref_pointer(x) })
    }

    pub fn is_signal_trampoline(&self) -> bool {
        self.fde.is_signal_trampoline()
    }

    pub fn language_specific_data(&self) -> Option<EndianSlice<'a, NativeEndian>> {
        // Safety: we have to trust the DWARF info here
        let addr = self.fde.lsda().map(|x| unsafe { deref_pointer(x) })?;

        Some(EndianSlice::new(
            // Safety: we have to trust the DWARF info here
            unsafe { get_unlimited_slice(addr as *const u8) },
            NativeEndian,
        ))
    }

    pub fn text_rel_base(&self) -> Option<u64> {
        obtain_eh_info().bases.eh_frame.text
    }

    pub fn data_rel_base(&self) -> Option<u64> {
        obtain_eh_info().bases.eh_frame.text
    }

    pub fn region_start(&self) -> u64 {
        self.fde.initial_address()
    }

    pub(crate) fn adjust_stack_for_args(&mut self) {
        let size = self.row.saved_args_size();
        self.regs[arch::SP] = self.regs[arch::SP].wrapping_add(usize::try_from(size).unwrap());
    }

    pub fn set_ip(&mut self, value: usize) {
        self.regs[self.return_address_register] = value;
    }

    pub fn set_reg(&mut self, reg: Register, value: usize) {
        self.regs[reg] = value;
    }

    /// Restore control to this frame.
    ///
    /// # Safety
    ///
    /// This method is *highly* unsafe because it installs this frames register context, **without
    /// any checking**. If used improperly, much terrible things will happen, big sadness.
    pub unsafe fn restore(self) -> ! {
        // Safety: caller has to ensure this is safe
        unsafe { arch::restore_context(&self.regs) }
    }

    fn from_context(regs: &arch::Registers, pc: usize) -> Result<Self, gimli::Error> {
        let eh_info = obtain_eh_info();

        let fde = eh_info.hdr.table().unwrap().fde_for_address(
            &eh_info.eh_frame,
            &eh_info.bases,
            pc as u64,
            EhFrame::cie_from_offset,
        )?;

        let mut unwinder = gimli::UnwindContext::new_in();

        let row = fde
            .unwind_info_for_address(&eh_info.eh_frame, &eh_info.bases, &mut unwinder, pc as u64)?
            .clone();

        Ok(Self {
            return_address_register: fde.cie().return_address_register(),
            fde,
            row,
            regs: regs.clone(),
        })
    }

    #[expect(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "numeric casts are all checked and behave as expected"
    )]
    fn unwind(&self) -> Result<arch::Registers, gimli::Error> {
        let row = &self.row;
        let mut new_regs = self.regs.clone();

        #[expect(clippy::match_wildcard_for_single_variants, reason = "style choice")]
        let cfa = match *row.cfa() {
            CfaRule::RegisterAndOffset { register, offset } => {
                self.regs[register].wrapping_add(offset as usize)
            }
            _ => return Err(gimli::Error::UnsupportedEvaluation),
        };

        new_regs[arch::SP] = cfa;

        debug_assert_eq!(self.return_address_register, arch::RA);
        new_regs[self.return_address_register] = 0;

        for reg in 0..arch::MAX_REG {
            let reg = Register(reg);

            let rule = row.register(reg);

            match rule {
                // According to LLVM libunwind (and this appears to be true in practice as well)
                // leaf functions don't actually store their return address on the stack instead
                // keeping it in register and there is no explicit EH_FRAME instruction on how to restore it.
                // (great btw that stuff like this is well documented - not)
                // This means if the register is the return address register AND there isn't a register
                // rule for it, we need to maintain it nonetheless.
                RegisterRule::Undefined if reg == self.return_address_register => {
                    new_regs[reg] = self.regs[self.return_address_register];
                }
                RegisterRule::Undefined => {}
                RegisterRule::SameValue => new_regs[reg] = self.regs[reg],
                // Safety: we have to trust the DWARF info here
                RegisterRule::Offset(offset) => unsafe {
                    new_regs[reg] = *(cfa.wrapping_add(offset as usize) as *const usize);
                },
                RegisterRule::ValOffset(offset) => {
                    new_regs[reg] = cfa.wrapping_add(offset as usize);
                }
                RegisterRule::Register(reg) => new_regs[reg] = self.regs[reg],
                RegisterRule::Expression(_) | RegisterRule::ValExpression(_) => {
                    return Err(gimli::Error::UnsupportedEvaluation)
                }
                RegisterRule::Architectural => unreachable!(),
                RegisterRule::Constant(value) => new_regs[reg] = usize::try_from(value).unwrap(),
                _ => unreachable!(),
            }
        }

        Ok(new_regs)
    }
}

#[derive(Clone)]
pub struct FramesIter {
    regs: arch::Registers,
    signal: bool,
    pc: usize,
    limit: usize,
}

impl Default for FramesIter {
    fn default() -> Self {
        Self::new()
    }
}

impl FramesIter {
    #[inline(always)]
    pub fn new() -> Self {
        with_context(|ctx, pc| Self {
            pc,
            regs: ctx.clone(),
            signal: false,
            limit: 64,
        })
    }

    pub fn from_registers(regs: arch::Registers, pc: usize) -> Self {
        Self {
            regs,
            signal: false,
            pc,
            limit: 64,
        }
    }
}

impl FallibleIterator for FramesIter {
    type Item = Frame<'static>;
    type Error = crate::Error;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        debug_assert!(self.limit > 0);
        let mut pc = self.pc;

        // The previous call to `Frame::unwind` set the return address to zero (meaning there was no
        // information on how to restore the return address) this means we're done walking the stack.
        // Reached end of stack
        if pc == 0 {
            return Ok(None);
        }

        // RA points to the *next* instruction, so move it back 1 byte for the call instruction.
        if !self.signal {
            pc -= 1;
        }

        let frame = Frame::from_context(&self.regs, pc)?;
        self.regs = frame.unwind()?;
        self.signal = frame.is_signal_trampoline();

        // Use the return address as the next value of `pc` this essentially simulates a
        // function return.
        self.pc = self.regs[arch::RA];

        self.limit -= 1;

        Ok(Some(frame))
    }
}
