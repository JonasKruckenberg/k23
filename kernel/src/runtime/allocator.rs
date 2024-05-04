// use crate::kconfig;
// use crate::kernel_mapper::with_frame_alloc;
// use core::alloc::{AllocError, GlobalAlloc, Layout};
// use core::ops::Range;
// use core::ptr::NonNull;
// use linked_list_allocator::LockedHeap;
// use vmm::{AddressRangeExt, EntryFlags, Flush, Mapper, Mode, VirtualAddress};
//
// /// An allocator for "guest-owned" memory.
// ///
// /// Keeps track of the guests address space and allocates ranges of
// /// virtual memory (backed by the global FRAME_ALLOCATOR).
// pub struct GuestAllocator {
//     asid: usize,
//     root_table: VirtualAddress,
//     virt_offset: VirtualAddress,
//     inner: LockedHeap,
// }
//
// impl GuestAllocator {
//     pub fn new(asid: usize, virt_offset: VirtualAddress) -> Self {
//         let root_table = kconfig::MEMORY_MODE::get_active_table(asid);
//         let root_table = kconfig::MEMORY_MODE::phys_to_virt(root_table);
//
//         let mut this = Self {
//             asid,
//             virt_offset,
//             root_table,
//             inner: LockedHeap::empty(),
//         };
//
//         let (mem_virt, flush) = this.map_additional_pages(3);
//         flush.flush().unwrap();
//
//         unsafe {
//             this.inner
//                 .lock()
//                 .init(mem_virt.start.as_raw() as *mut u8, mem_virt.size());
//         }
//
//         this
//     }
//
//     fn map_additional_pages(
//         &mut self,
//         num_pages: usize,
//     ) -> (Range<VirtualAddress>, Flush<kconfig::MEMORY_MODE>) {
//         with_frame_alloc(|alloc| {
//             let mut mapper = Mapper::from_address(self.asid, self.root_table, alloc);
//
//             let mem_phys = {
//                 let start = mapper.allocator_mut().allocate_frames(num_pages).unwrap();
//                 start..start.add(num_pages * kconfig::PAGE_SIZE)
//             };
//
//             let mem_virt = self.virt_offset..self.virt_offset.add(num_pages * kconfig::PAGE_SIZE);
//             self.virt_offset = mem_virt.end;
//
//             let flush = mapper
//                 .map_range(
//                     mem_virt.clone(),
//                     mem_phys,
//                     EntryFlags::READ | EntryFlags::WRITE,
//                 )
//                 .unwrap();
//
//             (mem_virt, flush)
//         })
//     }
// }
//
// unsafe impl alloc::alloc::Allocator for GuestAllocator {
//     fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
//         let ptr = unsafe { self.inner.alloc(layout) };
//
//         if let Some(ptr) = NonNull::new(ptr) {
//             Ok(NonNull::slice_from_raw_parts(ptr, layout.size()))
//         } else {
//             // TODO map new pages
//
//             Err(AllocError)
//         }
//     }
//
//     unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
//         // TODO unmap pages
//         self.inner.dealloc(ptr.cast().as_ptr(), layout)
//     }
// }
