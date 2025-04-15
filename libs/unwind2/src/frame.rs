// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch;
use crate::eh_info::obtain_eh_info;
use crate::utils::{StoreOnStack, deref_pointer, get_unlimited_slice, with_context};
use fallible_iterator::FallibleIterator;
use gimli::{
    CfaRule, EhFrame, EndianSlice, EvaluationResult, FrameDescriptionEntry, NativeEndian, Register,
    RegisterRule, UnwindExpression, UnwindSection, UnwindTableRow, Value,
};

/// A frame in a stack.
///
/// This holds all the information about a call frame.
#[derive(Debug)]
pub struct Frame<'a> {
    regs: arch::Registers,
    eh_frame: &'a EhFrame<EndianSlice<'a, NativeEndian>>,
    fde: FrameDescriptionEntry<EndianSlice<'a, NativeEndian>, usize>,
    row: UnwindTableRow<usize, StoreOnStack>,
    return_address_register: Register,
}

impl<'a> Frame<'a> {
    /// Returns the current instruction pointer of this frame.
    pub fn ip(&self) -> usize {
        self.regs[self.return_address_register]
    }

    /// Sets the value of this frames instruction pointer.
    ///
    /// When paired with [`Frame::restore`] this will transfer control to the instruction pointer.
    pub fn set_ip(&mut self, value: usize) {
        self.regs[self.return_address_register] = value;
    }

    /// Returns the current stack pointer of this frame.
    pub fn sp(&self) -> usize {
        self.regs[arch::SP]
    }

    /// Returns the starting symbol address of the frame of this function.
    pub fn symbol_address(&self) -> u64 {
        self.fde.initial_address()
    }

    /// Returns the address of the function's personality routine handler if any.
    ///
    /// This personality routine does language-specific clean up when unwinding the stack frames.
    /// Not all frames have a pointer to a personality routine defined, but for Rust `catch_unwind`
    /// and frames that need to perform `Drop` cleanup do.
    pub fn personality(&self) -> Option<u64> {
        // Safety: we have to trust the DWARF info here
        self.fde.personality().map(|x| unsafe { deref_pointer(x) })
    }

    /// Returns `true` if this Frame belongs to a signal trampoline handler.
    ///
    /// Usually the return address points to one past the actual call instruction since that
    /// is where we need to transfer control to when returning, but in the context of a signal
    /// handler (or a machine exception handler that doesn't use a separate exception stack)
    /// the frame that is pushed to the stack points its return address to the instruction we
    /// need to return to directly (since that is the instruction we interrupted).
    ///
    /// The return value of `is_signal_trampoline` should be used to adjust the address accordingly.
    ///
    /// Note that this is only useful *if* you actually use a shared stack for signal/trap handlers
    /// if you have separate stacks then you can disregard this information (and it is likely you
    /// don't have any frames marked as signal trampolines anyway).
    pub fn is_signal_trampoline(&self) -> bool {
        self.fde.is_signal_trampoline()
    }

    /// Return a pointer to the language specific data area LSDA.
    ///
    /// The format of this region is language dependent, but in Rusts case it holds information
    /// about whether the landing pad is a `catch_unwind` or a `Drop` cleanup impl.
    pub fn language_specific_data(&self) -> Option<EndianSlice<'a, NativeEndian>> {
        // Safety: we have to trust the DWARF info here
        let addr = self.fde.lsda().map(|x| unsafe { deref_pointer(x) })?;

        Some(EndianSlice::new(
            // Safety: we have to trust the DWARF info here
            unsafe { get_unlimited_slice(addr as *const u8) },
            NativeEndian,
        ))
    }

    /// Retrieve the value of the specified register.
    pub fn reg(&self, reg: Register) -> usize {
        self.regs[reg]
    }

    /// Sets the value of the specified register.
    ///
    /// Note that this will only update the representation in this frame not the actual machine register.
    /// To restore a frames register context see [`Frame::restore`].
    pub fn set_reg(&mut self, reg: Register, value: usize) {
        self.regs[reg] = value;
    }

    /// Restore control to this frame.
    ///
    /// # Safety
    ///
    /// This method is *highly* unsafe because it installs this frames register context, **without
    /// any checking**. If used improperly, much terrible things will happen, big sadness.
    //
    // You might have noticed that this restore operation never actually jumps anywhere and yet it
    // transfers control and is marked `!` how come?
    // Well this operation loads *all* registers saved in this frames register context into the
    // machine registers, importantly including the return address. When we then return from this
    // function, the return address will not point to the original caller anymore but to the address
    // we want to transfer control to.
    //
    // That's also why this function must never be inlined, if in between restoring the register
    // context calling `ret` we end up clobbering some registers that would obviously be bad and result
    // in very awkward to troubleshoot bugs.
    #[inline(never)]
    pub unsafe fn restore(self) -> ! {
        // Safety: caller has to ensure this is safe
        unsafe { arch::restore_context(&self.regs) }
    }

    pub(crate) fn text_rel_base(&self) -> Option<u64> {
        obtain_eh_info().bases.eh_frame.text
    }

    pub(crate) fn data_rel_base(&self) -> Option<u64> {
        obtain_eh_info().bases.eh_frame.data
    }

    pub(crate) fn adjust_stack_for_args(&mut self) {
        let size = self.row.saved_args_size();
        self.regs[arch::SP] = self.regs[arch::SP].wrapping_add(usize::try_from(size).unwrap());
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
            eh_frame: &eh_info.eh_frame,
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

        let cfa = match *row.cfa() {
            CfaRule::RegisterAndOffset { register, offset } => {
                self.regs[register].wrapping_add(offset as usize)
            }
            CfaRule::Expression(expr) => {
                let result = self.eval_expression(expr)?;
                result.unwrap().to_u64(u64::MAX)? as usize
            }
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
                RegisterRule::Expression(expr) => {
                    let result = self.eval_expression(expr)?;
                    let addr = result.unwrap().to_u64(u64::MAX)? as usize;
                    // Safety: we have to trust the DWARF info here
                    new_regs[reg] = unsafe { *(addr as *const usize) };
                }
                RegisterRule::ValExpression(expr) => {
                    let result = self.eval_expression(expr)?;
                    new_regs[reg] = result.unwrap().to_u64(u64::MAX)? as usize;
                }
                RegisterRule::Architectural => unreachable!(),
                RegisterRule::Constant(value) => new_regs[reg] = usize::try_from(value).unwrap(),
                _ => unreachable!(),
            }
        }

        Ok(new_regs)
    }

    fn eval_expression(&self, expr: UnwindExpression<usize>) -> gimli::Result<Option<Value>> {
        let expr = expr.get(self.eh_frame)?;

        let mut eval = expr.evaluation(self.fde.cie().encoding());
        let mut result = eval.evaluate()?;

        loop {
            result = match result {
                EvaluationResult::Complete => return Ok(eval.value_result()),
                EvaluationResult::RequiresRegister {
                    register,
                    base_type,
                } => {
                    assert_eq!(base_type.0, 0);
                    eval.resume_with_register(Value::Generic(self.regs[register] as u64))?
                }
                EvaluationResult::RequiresMemory {
                    address,
                    size,
                    space,
                    base_type,
                } => {
                    assert_eq!(size, 8);
                    assert!(space.is_none());
                    assert_eq!(base_type.0, 0);

                    // Safety: we have to trust the DWARF info here
                    let val = unsafe { (address as *mut u64).read() };
                    eval.resume_with_memory(Value::Generic(val))?
                }
                EvaluationResult::RequiresFrameBase => todo!(),
                EvaluationResult::RequiresTls(_) => todo!(),
                EvaluationResult::RequiresCallFrameCfa => todo!(),
                EvaluationResult::RequiresAtLocation(_) => todo!(),
                EvaluationResult::RequiresEntryValue(_) => todo!(),
                EvaluationResult::RequiresParameterRef(_) => todo!(),
                EvaluationResult::RequiresRelocatedAddress(_) => todo!(),
                EvaluationResult::RequiresIndexedAddress { .. } => todo!(),
                EvaluationResult::RequiresBaseType(_) => todo!(),
            }
        }
    }
}

/// An iterator over frames on the stack.
///
/// This is the primary means for walking the stack in `unwind2`.
///
/// ```rust
/// # use unwind2::FrameIter;
/// use fallible_iterator::FallibleIterator;
///
/// let mut frames = FrameIter::new(); // start the stack walking at the current frame
/// while let Some(frame) = frames.next().unwrap() { // FrameIter implements FallibleIterator
///     println!("ip: {:#x} sp: {:#x}", frame.ip(), frame.sp());
/// }
/// ```
///
///
/// You can also construct a `FrameIter` from the raw register context and instruction pointer:
///
/// ```rust
/// # use unwind2::FrameIter;
/// use fallible_iterator::FallibleIterator;
///
/// // in a real scenario you would obtain these values from e.g. a signal/trap handler
/// let regs = unwind2::Registers {gp: [0; 32],fp: [0; 32]};
/// let ip = 0;
///
/// let mut frames = FrameIter::from_registers(regs, ip);
/// while let Some(frame) = frames.next().unwrap() { // FrameIter implements FallibleIterator
///     println!("ip: {:#x} sp: {:#x}", frame.ip(), frame.sp());
/// }
/// ```
#[derive(Clone)]
pub struct FrameIter {
    regs: arch::Registers,
    signal: bool,
    ip: usize,
}

impl Default for FrameIter {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameIter {
    /// Construct a new `FrameIter` that will walk the stack beginning at the callsite.
    #[inline(always)]
    pub fn new() -> Self {
        with_context(|ctx, ip| Self {
            ip,
            regs: ctx.clone(),
            signal: false,
        })
    }

    /// Construct a new `FrameIter` that will walk the stack beginning at the provided context.
    ///
    /// The two most important values are the stack pointer and the instruction pointer.
    pub fn from_registers(regs: arch::Registers, ip: usize) -> Self {
        Self {
            regs,
            signal: false,
            ip,
        }
    }
}

impl FallibleIterator for FrameIter {
    type Item = Frame<'static>;
    type Error = crate::Error;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        let mut ip = self.ip;

        // The previous call to `Frame::unwind` set the return address to zero (meaning there was no
        // information on how to restore the return address) this means we're done walking the stack.
        // Reached end of stack
        if ip == 0 {
            return Ok(None);
        }

        // RA points to the *next* instruction, so move it back 1 byte for the call instruction.
        if !self.signal {
            ip -= 1;
        }

        // Construct the frame from the registers and instruction pointer, this will also look up
        // all the required unwind information.
        let frame = Frame::from_context(&self.regs, ip)?;
        // and then unwind the frame to obtain the next register context
        self.regs = frame.unwind()?;
        // Use the return address as the next value of `pc` this essentially simulates a
        // function return.
        self.ip = self.regs[arch::RA];

        self.signal = frame.is_signal_trampoline();

        Ok(Some(frame))
    }
}
