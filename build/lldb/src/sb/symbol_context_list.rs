use crate::{debug_descr, IsValid, SBIterator, SBSymbolContext};
use cpp::{cpp, cpp_class};
use std::fmt;

cpp_class!(pub unsafe struct SBSymbolContextList as "SBSymbolContextList");

unsafe impl Send for SBSymbolContextList {}

impl SBSymbolContextList {
    pub fn new() -> SBSymbolContextList {
        cpp!(unsafe [] -> SBSymbolContextList as "SBSymbolContextList" { return SBSymbolContextList(); })
    }
    pub fn len(&self) -> usize {
        cpp!(unsafe [self as "SBSymbolContextList*"] -> usize as "size_t" {
            return self->GetSize();
        })
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        cpp!(unsafe [self as "SBSymbolContextList*"] {
            return self->Clear();
        })
    }
    pub fn context_at_index(&self, index: u32) -> SBSymbolContext {
        cpp!(unsafe [self as "SBSymbolContextList*", index as "uint32_t"] -> SBSymbolContext as "SBSymbolContext" {
            return self->GetContextAtIndex(index);
        })
    }
    pub fn iter(&self) -> impl Iterator<Item = SBSymbolContext> + '_ {
        SBIterator::new(self.len() as u32, move |index| self.context_at_index(index))
    }
}

impl IsValid for SBSymbolContextList {
    fn is_valid(&self) -> bool {
        cpp!(unsafe [self as "SBSymbolContextList*"] -> bool as "bool" {
            return self->IsValid();
        })
    }
}

impl fmt::Debug for SBSymbolContextList {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        debug_descr(f, |descr| {
            cpp!(unsafe [self as "SBSymbolContextList*", descr as "SBStream*"] -> bool as "bool" {
                return self->GetDescription(*descr);
            })
        })
    }
}
