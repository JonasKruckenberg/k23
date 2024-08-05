use super::{arch, PersonalityRoutine};
use gimli::{
    BaseAddresses, CfaRule, EhFrame, FrameDescriptionEntry, Register, RegisterRule, UnwindTableRow,
};
use gimli::{EndianSlice, NativeEndian};

pub type StaticSlice = EndianSlice<'static, NativeEndian>;

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

impl<R: gimli::Reader> gimli::UnwindContextStorage<R> for StoreOnStack {
    type Rules = [(Register, RegisterRule<R>); next_value(arch::MAX_REG_RULES)];
    type Stack = [UnwindTableRow<R, Self>; 2];
}

#[derive(Debug)]
pub struct Frame<'a> {
    fde_result: &'static FDESearchResult,
    row: &'a UnwindTableRow<StaticSlice, StoreOnStack>,
}

#[derive(Debug)]
pub struct FDESearchResult {
    pub fde: FrameDescriptionEntry<StaticSlice>,
    pub bases: BaseAddresses,
    pub eh_frame: EhFrame<StaticSlice>,
}

impl<'a> Frame<'a> {
    pub fn from_context(ctx: &arch::Context, signal: bool) -> Result<Option<Self>, gimli::Error> {
        // TODO lazliy load eh_frame

        // let fde_result = match find_fde::get_finder().find_fde(ra as _) {
        //     Som<e(v) => v,
        //     None => return Ok(None),
        // };
        // let mut unwinder = UnwindContext::<_, StoreOnStack>::new_in();
        // let row = fde_result
        //     .fde
        //     .unwind_info_for_address(
        //         &fde_result.eh_frame,
        //         &fde_result.bases,
        //         &mut unwinder,
        //         ra as _,
        //     )?
        //     .clone();

        // Ok(Some(Self { fde_result, row }))

        todo!()
    }

    pub fn unwind(&self, ctx: &arch::Context) -> Result<arch::Context, gimli::Error> {
        let row = &self.row;
        let mut new_ctx = ctx.clone();

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
                    *((cfa.wrapping_add(offset as usize)) as *const usize)
                },
                RegisterRule::ValOffset(offset) => cfa.wrapping_add(offset as usize),
                RegisterRule::Expression(_) | RegisterRule::ValExpression(_) => {
                    return Err(gimli::Error::UnsupportedEvaluation)
                }
                RegisterRule::Architectural => unreachable!(),
                RegisterRule::Constant(value) => value as usize,
                _ => unreachable!(),
            };
            new_ctx[*reg] = value;
        }

        Ok(new_ctx)
    }

    pub fn personality(&self) -> Option<PersonalityRoutine> {
        self.fde_result
            .fde
            .personality()
            .map(|x| unsafe { deref_pointer(x) })
            .map(|x| unsafe { core::mem::transmute(x) })
    }

    pub fn is_signal_trampoline(&self) -> bool {
        self.fde_result.fde.is_signal_trampoline()
    }
}
