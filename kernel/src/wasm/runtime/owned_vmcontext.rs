use crate::arch;
use crate::vm::{AddressRangeExt, AddressSpace, AddressSpaceRegion, Permissions, VirtualAddress};
use crate::wasm::runtime::{VMContext, VMOffsets};
use alloc::string::ToString;
use core::alloc::Layout;
use core::range::Range;

#[derive(Debug)]
pub struct OwnedVMContext {
    range: Range<VirtualAddress>,
}

impl OwnedVMContext {
    #[expect(clippy::unnecessary_wraps, reason = "TODO")]
    pub fn try_new(
        aspace: &mut AddressSpace,
        offsets: &VMOffsets,
    ) -> crate::wasm::Result<OwnedVMContext> {
        let layout = Layout::from_size_align(offsets.size() as usize, arch::PAGE_SIZE).unwrap();

        let virt_range = aspace
            .map(
                layout,
                Permissions::READ | Permissions::WRITE,
                |range, flags, batch| {
                    let region =
                        AddressSpaceRegion::new_zeroed(range, flags, Some("VMCOntext".to_string()));

                    region.commit(batch, range, true)?;

                    Ok(region)
                },
            )
            .unwrap()
            .range;

        Ok(Self { range: virt_range })
    }
    pub fn as_ptr(&self) -> *const VMContext {
        self.range.start.as_ptr().cast()
    }
    pub fn as_mut_ptr(&mut self) -> *mut VMContext {
        self.range.start.as_mut_ptr().cast()
    }
    pub unsafe fn plus_offset<T>(&self, offset: u32) -> *const T {
        // Safety: caller has to ensure offset is valid
        unsafe {
            self.as_ptr()
                .byte_add(usize::try_from(offset).unwrap())
                .cast()
        }
    }
    pub unsafe fn plus_offset_mut<T>(&mut self, offset: u32) -> *mut T {
        // Safety: caller has to ensure offset is valid
        unsafe {
            self.as_mut_ptr()
                .byte_add(usize::try_from(offset).unwrap())
                .cast()
        }
    }
}
