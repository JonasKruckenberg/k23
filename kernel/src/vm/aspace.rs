use crate::arch;
use crate::vm::mapping::Mapping;
use crate::vm::PageFaultFlags;
use crate::Error;
use alloc::boxed::Box;
use alloc::string::String;
use core::alloc::Layout;
use core::cmp;
use core::num::{NonZero, NonZeroUsize};
use core::ops::Range;
use core::pin::Pin;
use core::ptr::NonNull;
use pmm::frame_alloc::BuddyAllocator;
use pmm::{AddressRangeExt, Flush, VirtualAddress};
use rand::distributions::Uniform;
use rand::Rng;
use rand_chacha::ChaCha20Rng;
use wavltree::Entry;

pub struct AddressSpace {
    pub tree: wavltree::WAVLTree<Mapping>,
    address_range: Range<VirtualAddress>,
    frame_alloc: BuddyAllocator,
    arch: pmm::AddressSpace,
    prng: Option<ChaCha20Rng>,
    last_fault: Option<NonNull<Mapping>>,
}

// TODO this isnt right
unsafe impl Send for AddressSpace {}

unsafe impl Sync for AddressSpace {}

impl AddressSpace {
    pub fn new_user(
        arch: pmm::AddressSpace,
        frame_alloc: BuddyAllocator,
        prng: ChaCha20Rng,
    ) -> Self {
        Self {
            tree: wavltree::WAVLTree::default(),
            address_range: arch::USER_ASPACE_BASE..VirtualAddress::MAX,
            frame_alloc,
            arch,
            prng: Some(prng),
            last_fault: None,
        }
    }

    pub fn new_kernel(
        arch: pmm::AddressSpace,
        frame_alloc: BuddyAllocator,
        prng: ChaCha20Rng,
    ) -> Self {
        Self {
            tree: wavltree::WAVLTree::default(),
            address_range: arch::KERNEL_ASPACE_BASE..VirtualAddress::MAX,
            frame_alloc,
            arch,
            prng: Some(prng),
            last_fault: None,
        }
    }
    
    
    
    
    
    
    
    
    
    
    
    
    
    

    // pub fn page_fault(&mut self, virt: VirtualAddress, flags: PageFaultFlags) -> crate::Result<()> {
    //     if flags.contains(PageFaultFlags::ACCESS) {
    //         return self.access_fault(virt);
    //     }
    // 
    //     let virt = virt.align_down(arch::PAGE_SIZE);
    // 
    //     // check if the address is within the last fault range
    //     // if so, we can save ourselves a tree lookup
    //     if let Some(mut last_fault) = self.last_fault {
    //         let last_fault = unsafe { Pin::new_unchecked(last_fault.as_mut()) };
    // 
    //         if last_fault.range.contains(&virt) {
    //             return last_fault.page_fault(virt, flags);
    //         }
    //     }
    // 
    //     // the address wasn't in the last fault range, so we need to look it up
    //     // and update the last fault range
    //     if let Some(mapping) = self.find_mapping(virt) {
    //         // TODO actually update self.last_fault here
    //         mapping.page_fault(virt, flags)
    //     } else {
    //         log::trace!("page fault at unmapped address {virt}");
    //         Err(Error::AccessDenied)
    //     }
    // }
    // 
    // fn find_mapping(&mut self, virt: VirtualAddress) -> Option<Pin<&mut Mapping>> {
    //     self.tree
    //         .range_mut(virt..virt)
    //         .find(|mapping| mapping.range.contains(&virt))
    // }
    // 
    // pub fn access_fault(&mut self, addr: VirtualAddress) -> crate::Result<()> {
    //     todo!()
    // }
    // 
    // pub fn identity_map(
    //     &mut self,
    //     vmo: (),
    //     vmo_offset: usize,
    //     flags: pmm::Flags,
    // ) {
    //     let virt = vmo.range.start.add(usize)..vmo.range.end;
    //     self.map()
    // }
    // 
    // /// Map an object into virtual memory
    // pub fn map(
    //     &mut self,
    //     range: Range<VirtualAddress>,
    //     flags: pmm::Flags,
    //     vmo: (),
    //     vmo_offset: (),
    // ) {
    //     todo!()
    // }
    // 
    // pub fn reserve(&mut self, range: Range<VirtualAddress>, flags: pmm::Flags, name: String) {
    //     // FIXME turn these into errors instead of panics
    //     match self.tree.entry(&range.start) {
    //         Entry::Occupied(_) => panic!("already reserved"),
    //         Entry::Vacant(mut entry) => {
    //             if let Some(next_mapping) = entry.peek_next_mut() {
    //                 assert!(range.end <= next_mapping.range.start,);
    // 
    //                 if next_mapping.range.start == range.end && next_mapping.flags == flags {
    //                     next_mapping.project().range.start = range.start;
    //                     return;
    //                 }
    //             }
    // 
    //             if let Some(prev_mapping) = entry.peek_prev_mut() {
    //                 assert!(prev_mapping.range.end <= range.start);
    // 
    //                 if prev_mapping.range.end == range.start && prev_mapping.flags == flags {
    //                     prev_mapping.project().range.end = range.end;
    //                     return;
    //                 }
    //             }
    // 
    //             entry.insert(Box::pin(Mapping::new(range, flags, name)));
    //         }
    //     }
    // }
    // 
    // pub fn unmap(&mut self, range: Range<VirtualAddress>) {
    //     let mut iter = self.tree.range_mut(range.clone());
    //     let mut flush = Flush::empty(self.arch.asid());
    // 
    //     while let Some(mapping) = iter.next() {
    //         log::trace!("{mapping:?}");
    //         let base = cmp::max(mapping.range.start, range.start);
    //         let len = cmp::min(mapping.range.end, range.end).sub_addr(base);
    // 
    //         if range.start <= mapping.range.start && range.end >= mapping.range.end {
    //             // this mappings range is entirely contained within `range`, so we need
    //             // fully remove the mapping from the tree
    //             // TODO verify if this absolutely insane code is actually any good
    // 
    //             let ptr = NonNull::from(mapping.get_mut());
    //             let mut cursor = unsafe { iter.tree().cursor_mut_from_ptr(ptr) };
    //             let mapping = cursor.remove().unwrap();
    // 
    //             self.arch
    //                 .unmap(
    //                     &mut self.frame_alloc,
    //                     mapping.range.start,
    //                     NonZero::new(mapping.range.size()).unwrap(),
    //                     &mut flush,
    //                 )
    //                 .unwrap();
    //         } else if range.start > mapping.range.start && range.end < mapping.range.end {
    //             // `range` is entirely contained within the mappings range, we
    //             // need to split the range in two
    //             let mapping = mapping.project();
    //             let left = mapping.range.start..range.start;
    // 
    //             mapping.range.start = range.end;
    //             iter.tree().insert(Box::pin(Mapping::new(
    //                 left,
    //                 *mapping.flags,
    //                 mapping.name.clone(),
    //             )));
    //         } else if range.start > mapping.range.start {
    //             // `range` is mostly past this mappings range, but overlaps partially
    //             // we need adjust the ranges end
    //             let mapping = mapping.project();
    //             mapping.range.end = range.start;
    //         } else if range.end < mapping.range.end {
    //             // `range` is mostly before this mappings range, but overlaps partially
    //             // we need adjust the ranges start
    //             let mapping = mapping.project();
    //             mapping.range.start = range.end;
    //         } else {
    //             unreachable!()
    //         }
    // 
    //         log::trace!("decommit {base:?}..{:?}", base.add(len));
    //         self.arch
    //             .unmap(
    //                 &mut self.frame_alloc,
    //                 base,
    //                 NonZeroUsize::new(len).unwrap(),
    //                 &mut flush,
    //             )
    //             .unwrap();
    //     }
    // 
    //     flush.flush().unwrap();
    // }
    // 
    // // behaviour:
    // //  - `range` must be fully mapped
    // //  - `new_flags` must be a subset of the current mappings flags (permissions can only be reduced)
    // //  - `range` must not be empty
    // //  - the above checks are done atomically ie they hold for all affected mappings
    // //  - if old and new flags are the same protect is a no-op
    // pub fn protect(&mut self, range: Range<VirtualAddress>, new_flags: pmm::Flags) {
    //     let iter = self.tree.range(range.clone());
    // 
    //     assert!(!range.is_empty());
    // 
    //     // check whether part of the range is not mapped, or the new flags are invalid for some mapping
    //     // in the range. If so, we need to terminate before actually materializing any changes
    //     let mut bytes_checked = 0;
    //     for mapping in iter {
    //         assert!(mapping.flags.contains(new_flags));
    //         bytes_checked += mapping.range.size();
    //     }
    //     assert_eq!(bytes_checked, range.size());
    // 
    //     // at this point we know the operation is valid, so can start updating the mappings
    //     let mut iter = self.tree.range_mut(range.clone());
    //     let mut flush = Flush::empty(self.arch.asid());
    // 
    //     while let Some(mapping) = iter.next() {
    //         // If the old and new flags are the same, nothing need to be materialized
    //         if mapping.flags == new_flags {
    //             continue;
    //         }
    // 
    //         if new_flags.is_empty() {
    //             let ptr = NonNull::from(mapping.get_mut());
    //             let mut cursor = unsafe { iter.tree().cursor_mut_from_ptr(ptr) };
    //             let mapping = cursor.remove().unwrap();
    // 
    //             self.arch
    //                 .unmap(
    //                     &mut self.frame_alloc,
    //                     mapping.range.start,
    //                     NonZero::new(mapping.range.size()).unwrap(),
    //                     &mut flush,
    //                 )
    //                 .unwrap();
    //         } else {
    //             let base = cmp::max(mapping.range.start, range.start);
    //             let len = NonZeroUsize::new(cmp::min(mapping.range.end, range.end).sub_addr(base))
    //                 .unwrap();
    // 
    //             self.arch.protect(base, len, new_flags, &mut flush).unwrap();
    //             *mapping.project().flags = new_flags;
    //         }
    //     }
    // 
    //     // synchronize the changes
    //     flush.flush().unwrap();
    // }
    // 
    // // pub fn commit(&mut self, range: Range<VirtualAddress>) {
    // //     todo!()
    // // }
    // //
    // // pub fn decommit(&mut self, range: Range<VirtualAddress>) {
    // //     todo!()
    // // }
    // 
    // // behaviour:
    // // - find the leftmost gap that satisfies the size and alignment requirements
    // //      - starting at the root,
    // pub fn find_spot(&mut self, layout: Layout, entropy: u8) -> VirtualAddress {
    //     log::trace!("finding spot for {layout:?} entropy {entropy}");
    // 
    //     let max_candidate_spaces: usize = 1 << entropy;
    //     log::trace!("max_candidate_spaces {max_candidate_spaces}");
    // 
    //     let selected_index: usize = self
    //         .prng
    //         .as_mut()
    //         .map(|prng| prng.sample(Uniform::new(0, max_candidate_spaces)))
    //         .unwrap_or_default();
    // 
    //     let spot = match self.find_spot_at_index(selected_index, layout) {
    //         Ok(spot) => spot,
    //         Err(0) => panic!("out of virtual memory"),
    //         Err(candidate_spot_count) => {
    //             log::trace!("couldn't find spot in first attempt (max_candidate_spaces {max_candidate_spaces}), retrying with (candidate_spot_count {candidate_spot_count})");
    //             let selected_index: usize = self
    //                 .prng
    //                 .as_mut()
    //                 .unwrap()
    //                 .sample(Uniform::new(0, candidate_spot_count));
    // 
    //             self.find_spot_at_index(selected_index, layout).unwrap()
    //         }
    //     };
    //     log::trace!("picked spot {spot:?}");
    // 
    //     spot
    // }
    // 
    // pub fn find_spot_at_index(
    //     &self,
    //     mut target_index: usize,
    //     layout: Layout,
    // ) -> Result<VirtualAddress, usize> {
    //     log::trace!("attempting to find spot for {layout:?} at index {target_index}");
    // 
    //     let spots_in_range = |layout: Layout, range: Range<VirtualAddress>| -> usize {
    //         ((range.size().saturating_sub(layout.size())) >> layout.align().ilog2()) + 1
    //     };
    // 
    //     let mut candidate_spot_count = 0;
    // 
    //     // see if there is a suitable gap between the start of the address space and the first mapping
    //     if let Some(root) = self.tree.root().get() {
    //         let gap_size = root.min_first_byte.sub_addr(self.address_range.start);
    //         let aligned_gap = self.address_range.start.align_up(layout.align())
    //             ..self
    //                 .address_range
    //                 .start
    //                 .add(gap_size)
    //                 .align_down(layout.align());
    //         let spot_count = spots_in_range(layout, aligned_gap.clone());
    //         candidate_spot_count += spot_count;
    //         if target_index < spot_count {
    //             return Ok(aligned_gap
    //                 .start
    //                 .add(target_index << layout.align().ilog2()));
    //         }
    //         target_index -= spot_count;
    //     }
    // 
    //     let mut maybe_node = self.tree.root().get();
    //     let mut already_visited = VirtualAddress::default();
    // 
    //     while let Some(node) = maybe_node {
    //         if node.max_gap >= layout.size() {
    //             if let Some(left) = node.links.left() {
    //                 let left = unsafe { left.as_ref() };
    // 
    //                 if left.max_gap >= layout.size() && left.max_last_byte > already_visited {
    //                     maybe_node = Some(left);
    //                     continue;
    //                 }
    // 
    //                 let gap_base = left.max_last_byte;
    //                 let gap_size = node.range.end.sub_addr(left.max_last_byte);
    //                 let aligned_gap = gap_base.align_up(layout.align())
    //                     ..gap_base.add(gap_size).align_down(layout.align());
    //                 let spot_count = spots_in_range(layout, aligned_gap.clone());
    // 
    //                 candidate_spot_count += spot_count;
    //                 if target_index < spot_count {
    //                     return Ok(aligned_gap
    //                         .start
    //                         .add(target_index << layout.align().ilog2()));
    //                 }
    //                 target_index -= spot_count;
    //             }
    // 
    //             if let Some(right) = node.links.right() {
    //                 let right = unsafe { right.as_ref() };
    // 
    //                 let gap_base = node.range.end;
    //                 let gap_size = right.min_first_byte.sub_addr(node.range.end);
    //                 let aligned_gap = gap_base.align_up(layout.align())
    //                     ..gap_base.add(gap_size).align_down(layout.align());
    //                 let spot_count = spots_in_range(layout, aligned_gap.clone());
    // 
    //                 candidate_spot_count += spot_count;
    //                 if target_index < spot_count {
    //                     return Ok(aligned_gap
    //                         .start
    //                         .add(target_index << layout.align().ilog2()));
    //                 }
    //                 target_index -= spot_count;
    // 
    //                 if right.max_gap >= layout.size() && right.max_last_byte > already_visited {
    //                     maybe_node = Some(right);
    //                     continue;
    //                 }
    //             }
    //         }
    //         already_visited = node.max_last_byte;
    //         maybe_node = node.links.parent().map(|ptr| unsafe { ptr.as_ref() });
    //     }
    // 
    //     // see if there is a suitable gap between the end of the last mapping and the end of the address space
    //     if let Some(root) = self.tree.root().get() {
    //         let gap_size = usize::MAX - root.max_last_byte.as_raw();
    //         let aligned_gap = root.max_last_byte.align_up(layout.align())
    //             ..root.max_last_byte.add(gap_size).align_down(layout.align());
    //         let spot_count = spots_in_range(layout, aligned_gap.clone());
    //         candidate_spot_count += spot_count;
    //         if target_index < spot_count {
    //             return Ok(aligned_gap
    //                 .start
    //                 .add(target_index << layout.align().ilog2()));
    //         }
    //         target_index -= spot_count;
    //     }
    // 
    //     Err(candidate_spot_count)
    // }
}
