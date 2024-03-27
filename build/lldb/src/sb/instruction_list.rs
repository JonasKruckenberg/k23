use crate::{debug_descr, IsValid, SBInstruction, SBIterator};
use cpp::{cpp, cpp_class};
use std::fmt;

cpp_class!(pub unsafe struct SBInstructionList as "SBInstructionList");

unsafe impl Send for SBInstructionList {}

impl SBInstructionList {
    pub fn len(&self) -> usize {
        cpp!(unsafe [self as "SBInstructionList*"] -> usize as "size_t" {
            return self->GetSize();
        })
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn instruction_at_index(&self, index: u32) -> SBInstruction {
        cpp!(unsafe [self as "SBInstructionList*", index as "uint32_t"] -> SBInstruction as "SBInstruction" {
            return self->GetInstructionAtIndex(index);
        })
    }
    pub fn iter(&self) -> impl Iterator<Item = SBInstruction> + '_ {
        SBIterator::new(self.len() as u32, move |index| {
            self.instruction_at_index(index)
        })
    }
}

impl IsValid for SBInstructionList {
    fn is_valid(&self) -> bool {
        cpp!(unsafe [self as "SBInstructionList*"] -> bool as "bool" {
            return self->IsValid();
        })
    }
}

impl fmt::Debug for SBInstructionList {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        debug_descr(f, |descr| {
            cpp!(unsafe [self as "SBInstructionList*", descr as "SBStream*"] -> bool as "bool" {
                return self->GetDescription(*descr);
            })
        })
    }
}
