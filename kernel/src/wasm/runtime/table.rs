use crate::vm::AddressSpace;
use crate::wasm::TABLE_MAX;
use crate::wasm::runtime::{MmapVec, VMFuncRef, VMTableDefinition};
use crate::wasm::translate::TableDesc;
use crate::wasm::utils::round_usize_up_to_host_pages;
use core::ptr::NonNull;

#[derive(Debug)]
pub struct Table {
    /// The underlying mmap-backed storage for this table.
    elements: MmapVec<Option<NonNull<VMFuncRef>>>,
    /// The optional maximum accessible size, in elements, for this table.
    maximum: Option<usize>,
}

impl Table {
    pub fn try_new(
        aspace: &mut AddressSpace,
        desc: &TableDesc,
        actual_maximum: Option<usize>,
    ) -> crate::wasm::Result<Self> {
        let reserve_size = TABLE_MAX.min(actual_maximum.unwrap_or(usize::MAX));

        let elements = if reserve_size == 0 {
            MmapVec::new_empty()
        } else {
            let mut elements = MmapVec::new_zeroed(aspace, reserve_size)?;
            elements.extend_with(aspace, usize::try_from(desc.minimum).unwrap(), None);
            elements
        };

        Ok(Self {
            elements,
            maximum: actual_maximum,
        })
    }

    pub fn elements_mut(&mut self) -> &mut [Option<NonNull<VMFuncRef>>] {
        self.elements.slice_mut()
    }
    pub(crate) fn as_vmtable_definition(&mut self) -> VMTableDefinition {
        VMTableDefinition {
            base: self.elements.as_mut_ptr().cast(),
            current_length: self.elements.len() as u64,
        }
    }
}
