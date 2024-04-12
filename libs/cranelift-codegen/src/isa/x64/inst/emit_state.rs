use super::*;

/// State carried between emissions of a sequence of instructions.
#[derive(Default, Clone, Debug)]
pub struct EmitState {
    /// Addend to convert nominal-SP offsets to real-SP offsets at the current
    /// program point.
    virtual_sp_offset: i64,
    /// Offset of FP from nominal-SP.
    nominal_sp_to_fp: i64,
    /// Safepoint stack map for upcoming instruction, as provided to `pre_safepoint()`.
    stack_map: Option<StackMap>,

    /// A copy of the frame layout, used during the emission of `Inst::ReturnCallKnown` and
    /// `Inst::ReturnCallUnknown` instructions.
    frame_layout: FrameLayout,
}

impl MachInstEmitState<Inst> for EmitState {
    fn new(abi: &Callee<X64ABIMachineSpec>) -> Self {
        EmitState {
            virtual_sp_offset: 0,
            nominal_sp_to_fp: abi.frame_size() as i64,
            stack_map: None,
            frame_layout: abi.frame_layout().clone(),
        }
    }

    fn pre_safepoint(&mut self, stack_map: StackMap) {
        self.stack_map = Some(stack_map);
    }
}

impl EmitState {
    pub(crate) fn take_stack_map(&mut self) -> Option<StackMap> {
        self.stack_map.take()
    }

    pub(crate) fn clear_post_insn(&mut self) {
        self.stack_map = None;
    }

    pub(crate) fn virtual_sp_offset(&self) -> i64 {
        self.virtual_sp_offset
    }

    pub(crate) fn adjust_virtual_sp_offset(&mut self, amount: i64) {
        let old = self.virtual_sp_offset;
        let new = self.virtual_sp_offset + amount;
        log::trace!("adjust virtual sp offset by {amount:#x}: {old:#x} -> {new:#x}",);
        self.virtual_sp_offset = new;
    }

    pub(crate) fn nominal_sp_to_fp(&self) -> i64 {
        self.nominal_sp_to_fp
    }

    pub(crate) fn frame_layout(&self) -> &FrameLayout {
        &self.frame_layout
    }
}
