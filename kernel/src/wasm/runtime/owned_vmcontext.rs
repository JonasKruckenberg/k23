use crate::arch;
use crate::vm::{
    frame_alloc, AddressRangeExt, AddressSpace, ArchAddressSpace, Batch, FrameList, VirtualAddress,
    Vmo,
};
use crate::wasm::runtime::{MmapVec, VMContext, VMOffsets};
use alloc::string::ToString;
use core::alloc::Layout;
use core::num::NonZeroUsize;
use core::range::Range;

#[derive(Debug)]
pub struct OwnedVMContext {
    frames: FrameList,
    range: Range<VirtualAddress>,
}

impl OwnedVMContext {
    #[expect(tail_expr_drop_order, reason = "")]
    #[expect(clippy::unnecessary_wraps, reason = "TODO")]
    pub fn try_new(
        aspace: &mut AddressSpace,
        offsets: &VMOffsets,
    ) -> crate::wasm::Result<OwnedVMContext> {
        let layout = Layout::from_size_align(offsets.size() as usize, arch::PAGE_SIZE).unwrap();
        let frames = frame_alloc::alloc_contiguous(layout.pad_to_align()).unwrap();

        log::trace!("{frames:?}");
        let phys_range = {
            let start = frames.first().unwrap().addr();
            Range::from(start..start.checked_add(layout.pad_to_align().size()).unwrap())
        };
        let vmo = Vmo::new_wired(phys_range);

        let virt_range = aspace
            .map(
                layout,
                vmo,
                0,
                crate::vm::Permissions::READ | crate::vm::Permissions::WRITE,
                Some("VMContext".to_string()),
            )
            .unwrap()
            .range;
        aspace.ensure_mapped(virt_range, true).unwrap();

        // let mut batch = Batch::new(&mut aspace.arch);
        // region.ensure_mapped(&mut batch, virt_range, true).unwrap();
        // batch.flush().unwrap();

        Ok(Self {
            frames,
            range: virt_range,
        })
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
