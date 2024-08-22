use super::{arch, eh_info::EH_INFO, utils::deref_pointer};
use crate::unwinder::PersonalityRoutine;
use core::fmt;
use core::fmt::Formatter;
use gimli::{
    BaseAddresses, CfaRule, EhFrame, EndianSlice, FrameDescriptionEntry, NativeEndian, Register,
    RegisterRule, UnwindContext, UnwindSection, UnwindTableRow,
};

pub struct Frame {
    fde: FrameDescriptionEntry<EndianSlice<'static, NativeEndian>, usize>,
    row: UnwindTableRow<usize, StoreOnStack>,
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("fde", &self.fde)
            .finish_non_exhaustive()
    }
}

impl Frame {
    pub fn from_context(ctx: &arch::Context, signal: bool) -> Result<Option<Self>, gimli::Error> {
        let mut ra = ctx[arch::RA];

        // Reached end of stack
        if ra == 0 {
            return Ok(None);
        }

        // RA points to the *next* instruction, so move it back 1 byte for the call instruction.
        if !signal {
            ra -= 1;
        }

        let fde = EH_INFO.hdr.table().unwrap().fde_for_address(
            &EH_INFO.eh_frame,
            &EH_INFO.bases,
            ra as u64,
            EhFrame::cie_from_offset,
        )?;

        let mut unwinder = UnwindContext::<_, StoreOnStack>::new_in();

        let row = fde
            .unwind_info_for_address(&EH_INFO.eh_frame, &EH_INFO.bases, &mut unwinder, ra as _)?
            .clone();

        Ok(Some(Self { fde, row }))
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn unwind(&self, ctx: &arch::Context) -> Result<arch::Context, gimli::Error> {
        let row = &self.row;
        let mut new_ctx = ctx.clone();

        #[allow(clippy::match_wildcard_for_single_variants)]
        let cfa = match *row.cfa() {
            CfaRule::RegisterAndOffset { register, offset } => {
                ctx[register].wrapping_add(offset as usize)
            }
            _ => return Err(gimli::Error::UnsupportedEvaluation),
        };

        new_ctx[arch::SP] = cfa as _;
        new_ctx[arch::RA] = 0;

        for (reg, rule) in row.registers() {
            let value = match *rule {
                RegisterRule::Undefined | RegisterRule::SameValue => ctx[*reg],
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

    #[allow(clippy::unused_self)]
    pub fn bases(&self) -> &BaseAddresses {
        &EH_INFO.bases
    }

    pub fn initial_address(&self) -> usize {
        usize::try_from(self.fde.initial_address()).unwrap()
    }

    pub fn personality(&self) -> Option<PersonalityRoutine> {
        self.fde
            .personality()
            .map(|x| unsafe { deref_pointer(x) })
            .map(|x| unsafe { core::mem::transmute(x) })
    }

    pub fn lsda(&self) -> usize {
        self.fde.lsda().map_or(0, |x| unsafe { deref_pointer(x) })
    }

    pub fn is_signal_trampoline(&self) -> bool {
        self.fde.is_signal_trampoline()
    }

    pub fn adjust_stack_for_args(&self, ctx: &mut arch::Context) {
        let size = self.row.saved_args_size();
        ctx[arch::SP] = ctx[arch::SP].wrapping_add(usize::try_from(size).unwrap());
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

impl<R: gimli::ReaderOffset> gimli::UnwindContextStorage<R> for StoreOnStack {
    type Rules = [(Register, RegisterRule<R>); next_value(arch::MAX_REG_RULES)];
    type Stack = [UnwindTableRow<R, Self>; 2];
}
