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
    ctx: arch::Context,
    fde: FrameDescriptionEntry<EndianSlice<'a, NativeEndian>, usize>,
    row: UnwindTableRow<usize, StoreOnStack>,
}

impl<'a> Frame<'a> {
    pub fn ip(&self) -> usize {
        self.ctx[arch::RA]
    }

    pub fn sp(&self) -> usize {
        self.ctx[arch::SP]
    }

    pub fn personality(&self) -> Option<u64> {
        self.fde.personality().map(|x| unsafe { deref_pointer(x) })
    }

    pub fn is_signal_trampoline(&self) -> bool {
        self.fde.is_signal_trampoline()
    }

    pub fn language_specific_data(&self) -> Option<EndianSlice<'a, NativeEndian>> {
        let addr = self.fde.lsda().map(|x| unsafe { deref_pointer(x) })?;

        Some(EndianSlice::new(
            unsafe { get_unlimited_slice(addr as _) },
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
        self.ctx[arch::SP] = self.ctx[arch::SP].wrapping_add(usize::try_from(size).unwrap());
    }

    pub fn set_ip(&mut self, value: usize) {
        self.ctx[arch::RA] = value;
    }

    pub fn set_reg(&mut self, reg: Register, value: usize) {
        self.ctx[reg] = value;
    }

    /// Restore control to this frame.
    ///
    /// # Safety
    ///
    /// This method is *highly* unsafe because it installs this frames register context, **without
    /// any checking**. If used improperly, much terrible things will happen, big sadness.
    pub unsafe fn restore(self) -> ! {
        arch::restore_context(&self.ctx)
    }

    fn from_context(ctx: &arch::Context, signal: bool) -> Result<Option<Self>, gimli::Error> {
        let mut ra = ctx[arch::RA];

        // Reached end of stack
        if ra == 0 {
            return Ok(None);
        }

        // RA points to the *next* instruction, so move it back 1 byte for the call instruction.
        if !signal {
            ra -= 1;
        }

        let eh_info = obtain_eh_info();

        let fde = eh_info.hdr.table().unwrap().fde_for_address(
            &eh_info.eh_frame,
            &eh_info.bases,
            ra as u64,
            EhFrame::cie_from_offset,
        )?;

        let mut unwinder = gimli::UnwindContext::new_in();

        let row = fde
            .unwind_info_for_address(&eh_info.eh_frame, &eh_info.bases, &mut unwinder, ra as _)?
            .clone();

        Ok(Some(Self {
            fde,
            row,
            ctx: ctx.clone(),
        }))
    }

    fn unwind(&self) -> Result<arch::Context, gimli::Error> {
        let row = &self.row;
        let mut new_ctx = self.ctx.clone();

        #[allow(clippy::match_wildcard_for_single_variants)]
        let cfa = match *row.cfa() {
            CfaRule::RegisterAndOffset { register, offset } => {
                self.ctx[register].wrapping_add(offset as usize)
            }
            _ => return Err(gimli::Error::UnsupportedEvaluation),
        };

        new_ctx[arch::SP] = cfa as _;
        new_ctx[arch::RA] = 0;

        for (reg, rule) in row.registers() {
            let value = match *rule {
                RegisterRule::Undefined | RegisterRule::SameValue => self.ctx[*reg],
                RegisterRule::Offset(offset) => unsafe {
                    *(cfa.wrapping_add(offset as usize) as *const usize)
                },
                RegisterRule::ValOffset(offset) => cfa.wrapping_add(offset as usize),
                RegisterRule::Expression(_) | RegisterRule::ValExpression(_) => {
                    return Err(gimli::Error::UnsupportedEvaluation)
                }
                RegisterRule::Constant(value) => usize::try_from(value).unwrap(),
                _ => unreachable!(),
            };
            new_ctx[*reg] = value;
        }

        Ok(new_ctx)
    }
}

#[derive(Clone)]
pub struct FramesIter {
    ctx: arch::Context,
    signal: bool,
}

impl FramesIter {
    #[inline(always)]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        with_context(|ctx| Self {
            ctx: ctx.clone(),
            signal: false,
        })
    }

    pub fn from_context(ctx: arch::Context) -> Self {
        Self { ctx, signal: false }
    }
}

impl FallibleIterator for FramesIter {
    type Item = Frame<'static>;
    type Error = crate::Error;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(frame) = Frame::from_context(&self.ctx, self.signal)? {
            self.ctx = frame.unwind()?;
            self.signal = frame.is_signal_trampoline();
            Ok(Some(frame))
        } else {
            Ok(None)
        }
    }
}
