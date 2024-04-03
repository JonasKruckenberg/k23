use crate::ir::ValueLabel;
use crate::machinst::Reg;
use alloc::vec::Vec;
use hashbrown::HashMap;

/// Value location range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueLocRange {
    /// The ValueLoc containing a ValueLabel during this range.
    pub loc: LabelValueLoc,
    /// The start of the range. It is an offset in the generated code.
    pub start: u32,
    /// The end of the range. It is an offset in the generated code.
    pub end: u32,
}

/// The particular location for a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelValueLoc {
    /// Register.
    Reg(Reg),
    /// Offset from the Canonical Frame Address (aka CFA).
    CFAOffset(i64),
}

/// Resulting map of Value labels and their ranges/locations.
pub type ValueLabelsRanges = HashMap<ValueLabel, Vec<ValueLocRange>>;