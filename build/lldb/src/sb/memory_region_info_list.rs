use crate::{debug_descr, IsValid, SBIterator, SBMemoryRegionInfo};
use cpp::{cpp, cpp_class};
use std::fmt;

cpp_class!(pub unsafe struct SBMemoryRegionInfoList as "SBMemoryRegionInfoList");

unsafe impl Send for SBMemoryRegionInfoList {}

impl SBMemoryRegionInfoList {
    pub fn len(&self) -> usize {
        cpp!(unsafe [self as "SBMemoryRegionInfoList*"] -> usize as "size_t" {
            return self->GetSize();
        })
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        cpp!(unsafe [self as "SBMemoryRegionInfoList*"] {
            return self->Clear();
        })
    }

    pub fn memory_region_at_index(&self, index: u32) -> SBMemoryRegionInfo {
        let region_info = SBMemoryRegionInfo::new();
        let present = cpp!(unsafe [self as "SBMemoryRegionInfoList*", index as "uint32_t", region_info as "SBMemoryRegionInfo*"] -> bool as "bool" {
            return self->GetMemoryRegionAtIndex(index, *region_info);
        });
        assert!(present);
        region_info
    }

    pub fn iter(&self) -> impl Iterator<Item = SBMemoryRegionInfo> + '_ {
        SBIterator::new(self.len() as u32, move |index| {
            self.memory_region_at_index(index)
        })
    }
}
