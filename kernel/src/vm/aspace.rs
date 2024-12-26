use crate::arch;
use crate::vm::mapping::Mapping;
use crate::vm::{PageFaultFlags, Vmo, WiredVmo, FRAME_ALLOC};
use crate::Error;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use bitflags::Flags;
use core::alloc::Layout;
use core::cmp;
use core::num::{NonZero, NonZeroUsize};
use core::ops::{DerefMut, Range, RangeBounds};
use core::pin::Pin;
use core::ptr::NonNull;
use mmu::frame_alloc::{BuddyAllocator, FrameAllocator, FramesIterator};
use mmu::{AddressRangeExt, Flush, PhysicalAddress, VirtualAddress};
use rand::distributions::Uniform;
use rand::Rng;
use rand_chacha::ChaCha20Rng;
use sync::{Mutex, MutexGuard};
use wavltree::Entry;

// const VIRT_ALLOC_ENTROPY: u8 = u8::try_from((arch::VIRT_ADDR_BITS - arch::PAGE_SHIFT as u32) + 1).unwrap();
const VIRT_ALLOC_ENTROPY: u8 = 27;

pub struct AddressSpace {
    pub tree: wavltree::WAVLTree<Mapping>,
    address_range: Range<VirtualAddress>,
    pub mmu: Arc<Mutex<mmu::AddressSpace>>,
    prng: Option<ChaCha20Rng>,
    last_fault: Option<NonNull<Mapping>>,
    empty_vmo: Option<Arc<dyn Vmo>>,
}

// TODO this isnt right
unsafe impl Send for AddressSpace {}

unsafe impl Sync for AddressSpace {}

impl AddressSpace {
    pub fn new_user(arch: mmu::AddressSpace, prng: ChaCha20Rng) -> Self {
        Self {
            tree: wavltree::WAVLTree::default(),
            address_range: arch::USER_ASPACE_BASE..VirtualAddress::MAX,
            mmu: Arc::new(Mutex::new(arch)),
            prng: Some(prng),
            last_fault: None,
            empty_vmo: None,
        }
    }

    pub fn new_kernel(arch: mmu::AddressSpace, prng: ChaCha20Rng) -> Self {
        Self {
            tree: wavltree::WAVLTree::default(),
            address_range: arch::KERNEL_ASPACE_BASE..VirtualAddress::MAX,
            mmu: Arc::new(Mutex::new(arch)),
            prng: Some(prng),
            last_fault: None,
            empty_vmo: None,
        }
    }

    pub fn begin_batch(&self) -> Batch {
        Batch {
            mmu: self.mmu.clone(),
            range: Default::default(),
            flags: mmu::Flags::empty(),
            phys: vec![],
        }
    }

    /// Crate a new `Mapping` in this address space.
    ///
    /// The mapping will be placed at a chosen spot in the address space that
    /// satisfies the given `layout` requirements.
    /// It's memory will be backed by the provided `vmo` at the given `vmo_offset`.
    ///
    /// # ASLR
    ///
    /// When address space layout randomization (ASLR) is enabled, the spot will be chosen
    /// randomly from a set of candidate spots. The number of candidate spots is determined by the
    /// `entropy` config. (TODO make actual config)
    pub fn create_mapping(
        &mut self,
        batch: &mut Batch,
        layout: Layout,
        vmo: Arc<dyn Vmo>,
        vmo_offset: usize,
        flags: mmu::Flags,
        name: String,
    ) -> crate::Result<Pin<&mut Mapping>> {
        let base = self.find_spot(layout, VIRT_ALLOC_ENTROPY);
        let virt = base..base.add(layout.size());

        let mapping = self.tree.insert(Mapping::new(
            self.mmu.clone(),
            virt.clone(),
            flags,
            vmo,
            vmo_offset,
            name,
        ));
        mapping.map_range(batch, virt)?;

        Ok(mapping)
    }

    /// Create a new `Mapping` at the provided range in this address space.
    ///
    /// It's memory will be backed by the provided `vmo` at the given `vmo_offset`.
    pub fn create_mapping_specific(
        &mut self,
        batch: &mut Batch,
        virt: Range<VirtualAddress>,
        vmo: Arc<dyn Vmo>,
        vmo_offset: usize,
        flags: mmu::Flags,
        name: String,
    ) -> crate::Result<Pin<&mut Mapping>> {
        assert!(virt.start.is_aligned(arch::PAGE_SIZE));
        assert!(virt.end.is_aligned(arch::PAGE_SIZE));
        assert_eq!(vmo_offset % arch::PAGE_SIZE, 0);

        if let Some(prev) = self.tree.upper_bound(virt.start_bound()).get() {
            assert!(prev.range.end <= virt.start);
        }

        // TODO can we reuse the cursor we previously created for this?
        let mapping = self.tree.insert(Mapping::new(
            self.mmu.clone(),
            virt.clone(),
            flags,
            vmo,
            vmo_offset,
            name,
        ));
        mapping.map_range(batch, virt)?;

        Ok(mapping)
    }

    pub fn reserve(
        &mut self,
        range: Range<VirtualAddress>,
        flags: mmu::Flags,
        name: String,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        log::trace!("reserving {range:?} with flags {flags:?} and name {name}");

        let vmo = self
            .empty_vmo
            .get_or_insert_with(|| {
                WiredVmo::new(PhysicalAddress::default()..PhysicalAddress::default())
            })
            .clone();

        let mapping = self.tree.insert(Mapping::new(
            self.mmu.clone(),
            range.clone(),
            flags,
            vmo,
            0,
            name,
        ));

        let mut mmu_aspace = self.mmu.lock();
        let mut frame_alloc = FRAME_ALLOC.get().unwrap().lock();

        if flags.is_empty() {
            log::trace!("calling mmu_aspace.unmap({range:?}, {flags:?})");
            mmu_aspace.unmap(
                frame_alloc.deref_mut(),
                range.start,
                NonZeroUsize::new(range.size()).unwrap(),
                flush,
            )?;
        } else {
            mmu_aspace.protect(
                range.start,
                NonZeroUsize::new(range.size()).unwrap(),
                flags,
                flush,
            )?;
        }

        Ok(())
    }

    pub fn page_fault(&mut self, virt: VirtualAddress, flags: PageFaultFlags) -> crate::Result<()> {
        if flags.contains(PageFaultFlags::ACCESS) {
            return self.access_fault(virt);
        }

        let virt = virt.align_down(arch::PAGE_SIZE);

        // check if the address is within the last fault range
        // if so, we can save ourselves a tree lookup
        if let Some(mut last_fault) = self.last_fault {
            let last_fault = unsafe { Pin::new_unchecked(last_fault.as_mut()) };

            if last_fault.range.contains(&virt) {
                return last_fault.page_fault(virt, flags);
            }
        }

        // the address wasn't in the last fault range, so we need to look it up
        // and update the last fault range
        if let Some(mapping) = self.find_mapping(virt) {
            // TODO actually update self.last_fault here
            mapping.page_fault(virt, flags)
        } else {
            log::trace!("page fault at unmapped address {virt}");
            Err(Error::AccessDenied)
        }
    }

    pub fn access_fault(&mut self, addr: VirtualAddress) -> crate::Result<()> {
        todo!()
    }

    fn find_mapping(&mut self, virt: VirtualAddress) -> Option<Pin<&mut Mapping>> {
        self.tree
            .range_mut(virt..virt)
            .find(|mapping| mapping.range.contains(&virt))
    }

    /// Find a spot in the address space that satisfies the given `layout` requirements.
    ///
    /// This function will walk the ordered set of `Mappings` from left to right, looking for a gap
    /// that is large enough to fit the given `layout`.
    ///
    /// To enable ASLR we additionally choose a random `target_index` and require that the chosen
    /// gap is at lest the `target_index`th gap in the address space. The `target_index` is chosen
    /// in the range [0, 2^entropy).
    /// `entropy` is a configurable value, but by default it is set to `arch::VIRT_ADDR_BITS - arch::PAGE_SHIFT + 1`
    /// which is the number of usable bits when allocating virtual memory addresses. `arch::VIRT_ADDR_BITS`
    /// is the total number of usable bits in a virtual address, and `arch::PAGE_SHIFT` is the number
    /// of bits that are "lost" to used because all addresses must be at least page aligned.
    ///
    /// If the algorithm fails to find a suitable spot in the first attempt, it will have collected the
    /// total number of candidate spots and retry with a new `target_index` in the range [0, candidate_spot_count)
    /// which guarantees that a spot will be found as long as `candidate_spot_count > 0`.
    pub fn find_spot(&mut self, layout: Layout, entropy: u8) -> VirtualAddress {
        // behaviour:
        // - find the leftmost gap that satisfies the size and alignment requirements
        //      - starting at the root,
        log::trace!("finding spot for {layout:?} entropy {entropy}");

        let max_candidate_spaces: usize = 1 << entropy;
        log::trace!("max_candidate_spaces {max_candidate_spaces}");

        let selected_index: usize = self
            .prng
            .as_mut()
            .map(|prng| prng.sample(Uniform::new(0, max_candidate_spaces)))
            .unwrap_or_default();

        let spot = match self.find_spot_at_index(selected_index, layout) {
            Ok(spot) => spot,
            Err(0) => panic!("out of virtual memory"),
            Err(candidate_spot_count) => {
                log::trace!("couldn't find spot in first attempt (max_candidate_spaces {max_candidate_spaces}), retrying with (candidate_spot_count {candidate_spot_count})");
                let selected_index: usize = self
                    .prng
                    .as_mut()
                    .unwrap()
                    .sample(Uniform::new(0, candidate_spot_count));

                self.find_spot_at_index(selected_index, layout).unwrap()
            }
        };
        log::trace!("picked spot {spot:?}");

        spot
    }

    fn find_spot_at_index(
        &self,
        mut target_index: usize,
        layout: Layout,
    ) -> Result<VirtualAddress, usize> {
        log::trace!("attempting to find spot for {layout:?} at index {target_index}");

        let spots_in_range = |layout: Layout, range: Range<VirtualAddress>| -> usize {
            // ranges passed in here can become empty for a number of reasons (aligning might produce ranges
            // where end > start, or the range might be empty to begin with) in either case an empty
            // range means no spots are available
            if range.is_empty() {
                return 0;
            }
            ((range.size().saturating_sub(layout.size())) >> layout.align().ilog2()) + 1
        };

        let mut candidate_spot_count = 0;

        // see if there is a suitable gap between the start of the address space and the first mapping
        if let Some(root) = self.tree.root().get() {
            let aligned_gap =
                (self.address_range.start..root.min_first_byte).align_in(layout.align());
            let spot_count = spots_in_range(layout, aligned_gap.clone());
            candidate_spot_count += spot_count;
            if target_index < spot_count {
                return Ok(aligned_gap
                    .start
                    .add(target_index << layout.align().ilog2()));
            }
            target_index -= spot_count;
        }

        let mut maybe_node = self.tree.root().get();
        let mut already_visited = VirtualAddress::default();

        while let Some(node) = maybe_node {
            if node.max_gap >= layout.size() {
                if let Some(left) = node.links.left() {
                    let left = unsafe { left.as_ref() };

                    if left.max_gap >= layout.size() && left.max_last_byte > already_visited {
                        maybe_node = Some(left);
                        continue;
                    }

                    let aligned_gap = (left.max_last_byte..node.range.end).align_in(layout.align());
                    let spot_count = spots_in_range(layout, aligned_gap.clone());

                    candidate_spot_count += spot_count;
                    if target_index < spot_count {
                        return Ok(aligned_gap
                            .start
                            .add(target_index << layout.align().ilog2()));
                    }
                    target_index -= spot_count;
                }

                if let Some(right) = node.links.right() {
                    let right = unsafe { right.as_ref() };

                    let aligned_gap =
                        (node.range.end..right.min_first_byte).align_in(layout.align());
                    let spot_count = spots_in_range(layout, aligned_gap.clone());

                    candidate_spot_count += spot_count;
                    if target_index < spot_count {
                        return Ok(aligned_gap
                            .start
                            .add(target_index << layout.align().ilog2()));
                    }
                    target_index -= spot_count;

                    if right.max_gap >= layout.size() && right.max_last_byte > already_visited {
                        maybe_node = Some(right);
                        continue;
                    }
                }
            }
            already_visited = node.max_last_byte;
            maybe_node = node.links.parent().map(|ptr| unsafe { ptr.as_ref() });
        }

        // see if there is a suitable gap between the end of the last mapping and the end of the address space
        if let Some(root) = self.tree.root().get() {
            let aligned_gap = (root.max_last_byte..self.address_range.end).align_in(layout.align());
            let spot_count = spots_in_range(layout, aligned_gap.clone());
            candidate_spot_count += spot_count;
            if target_index < spot_count {
                return Ok(aligned_gap
                    .start
                    .add(target_index << layout.align().ilog2()));
            }
            target_index -= spot_count;
        }

        Err(candidate_spot_count)
    }

    // pub fn identity_map(
    //     &mut self,
    //     vmo: (),
    //     vmo_offset: usize,
    //     flags: mmu::Flags,
    // ) {
    //     let virt = vmo.range.start.add(usize)..vmo.range.end;
    //     self.map()
    // }
    //
    // /// Map an object into virtual memory
    // pub fn map(
    //     &mut self,
    //     range: Range<VirtualAddress>,
    //     flags: mmu::Flags,
    //     vmo: (),
    //     vmo_offset: (),
    // ) {
    //     todo!()
    // }

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
    // pub fn protect(&mut self, range: Range<VirtualAddress>, new_flags: mmu::Flags) {
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
}

pub struct Batch {
    mmu: Arc<Mutex<mmu::AddressSpace>>,
    range: Range<VirtualAddress>,
    flags: mmu::Flags,
    phys: Vec<(PhysicalAddress, usize)>,
}

impl Drop for Batch {
    fn drop(&mut self) {
        if !self.phys.is_empty() {
            log::error!("batch was not flushed before dropping");
            // panic_unwind::panic_in_drop!("batch was not flushed before dropping");
        }
    }
}

impl Batch {
    pub fn append(
        &mut self,
        base: VirtualAddress,
        phys: (PhysicalAddress, usize),
        flags: mmu::Flags,
    ) -> crate::Result<()> {
        log::trace!("appending {phys:?} at {base:?} with flags {flags:?}");
        if !self.can_append(base) || self.flags != flags {
            self.flush()?;
            self.flags = flags;
            self.range = base..base.add(phys.1);
        } else {
            self.range.end = self.range.end.add(phys.1);
        }

        self.phys.push(phys);

        Ok(())
    }

    pub fn flush(&mut self) -> crate::Result<()> {
        log::trace!("flushing batch {:?} {:?}...", self.range, self.phys);
        if self.phys.is_empty() {
            return Ok(());
        }

        let mut mmu = self.mmu.lock();
        let mut flush = Flush::empty(mmu.asid());
        let iter = BatchFramesIter {
            iter: self.phys.drain(..),
            alloc: FRAME_ALLOC.get().unwrap().lock(),
        };
        mmu.map(self.range.start, iter, self.flags, &mut flush)?;

        self.range = self.range.end..self.range.end;

        Ok(())
    }

    pub fn ignore(&mut self) {
        self.phys.clear();
    }

    fn can_append(&self, virt: VirtualAddress) -> bool {
        self.range.end == virt
    }
}

struct BatchFramesIter<'a> {
    iter: vec::Drain<'a, (PhysicalAddress, usize)>,
    alloc: MutexGuard<'a, BuddyAllocator>,
}
impl FramesIterator for BatchFramesIter<'_> {
    fn alloc_mut(&mut self) -> &mut dyn FrameAllocator {
        self.alloc.deref_mut()
    }
}
impl Iterator for BatchFramesIter<'_> {
    type Item = (PhysicalAddress, usize);

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}
