use core::convert::Infallible;
use core::ops::Range;

use crate::arch::PageTableEntry;
use crate::{Arch, MemoryAttributes, PageTableLevel, PhysicalAddress, VirtualAddress, Visit};

pub struct LookupVisitor {
    result: Option<(PhysicalAddress, MemoryAttributes, &'static PageTableLevel)>,
}

impl LookupVisitor {
    pub const fn new() -> Self {
        Self { result: None }
    }

    pub fn into_result(
        self,
    ) -> Option<(PhysicalAddress, MemoryAttributes, &'static PageTableLevel)> {
        self.result
    }
}

impl<A: Arch> Visit<A> for LookupVisitor {
    type Error = Infallible;

    fn visit_entry(
        &mut self,
        entry: A::PageTableEntry,
        level: &'static PageTableLevel,
        _range: Range<VirtualAddress>,
        _arch: &A,
    ) -> Result<(), Self::Error> {
        if entry.is_leaf() {
            self.result = Some((entry.address(), entry.attributes(), level));
        }

        Ok(())
    }
}
