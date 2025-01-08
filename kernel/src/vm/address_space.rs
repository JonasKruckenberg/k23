use crate::arch;
use crate::error::Error;
use crate::frame_alloc::FrameAllocator;
use crate::vm::address_space_region::AddressSpaceRegion;
use crate::vm::{PageFaultFlags, Permissions, Vmo, WiredVmo};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::alloc::Layout;
use core::ops::Bound;
use core::pin::Pin;
use core::range::Range;
use mmu::arch::PAGE_SIZE;
use mmu::{AddressRangeExt, PhysicalAddress, VirtualAddress};
use rand::distributions::Uniform;
use rand::Rng;
use rand_chacha::ChaCha20Rng;

// const VIRT_ALLOC_ENTROPY: u8 = u8::try_from((arch::VIRT_ADDR_BITS - arch::PAGE_SHIFT as u32) + 1).unwrap();
const VIRT_ALLOC_ENTROPY: u8 = 27;

/// Represents the address space of a process (or the kernel).
pub struct AddressSpace {
    /// A binary search tree of regions that make up this address space.
    pub(crate) regions: wavltree::WAVLTree<AddressSpaceRegion>,
    /// The hardware address space backing this "logical" address space that changes need to be
    /// materialized into in order to take effect.
    hw_aspace: (),
    /// The maximum range this address space can encompass.
    ///
    /// This is used to check new mappings against and speed up page fault handling.
    max_range: Range<VirtualAddress>,
    /// The pseudo-random number generator used for address space layout randomization or `None`
    /// if ASLR is disabled.
    prng: Option<ChaCha20Rng>,
    /// "Empty" placeholder VMO to back regions created by `reserve`
    placeholder_vmo: Option<Arc<Vmo>>,
}

impl AddressSpace {
    pub fn new_user(/*arch: mmu::AddressSpace,*/ prng: ChaCha20Rng) -> Self {
        Self {
            regions: wavltree::WAVLTree::default(),
            max_range: Range::from(arch::USER_ASPACE_BASE..VirtualAddress::MAX),
            hw_aspace: (),
            prng: Some(prng),
            placeholder_vmo: None,
        }
    }

    pub fn new_kernel(/*arch: mmu::AddressSpace,*/ prng: Option<ChaCha20Rng>) -> Self {
        Self {
            regions: wavltree::WAVLTree::default(),
            max_range: Range::from(arch::KERNEL_ASPACE_BASE..VirtualAddress::MAX),
            hw_aspace: (),
            prng,
            placeholder_vmo: None,
        }
    }

    pub fn map(
        &mut self,
        layout: Layout,
        vmo: Arc<Vmo>,
        vmo_offset: usize,
        permissions: Permissions,
        name: String,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        let base = self.find_spot(layout, VIRT_ALLOC_ENTROPY);
        let virt = Range::from(base..base.checked_add(layout.size()).unwrap());

        let region = self.regions.insert(AddressSpaceRegion::new(
            // self.mmu.clone(),
            virt,
            permissions,
            vmo,
            vmo_offset,
            name,
        ));
        // mapping.map_range(batch, virt)?;

        Ok(region)
    }

    pub fn map_specific(&mut self) {
        todo!()
    }

    pub fn reserve(
        &mut self,
        range: Range<VirtualAddress>,
        permissions: Permissions,
        name: String,
    ) -> crate::Result<()> {
        log::trace!("reserving {range:?} with flags {permissions:?} and name {name}");

        let vmo = self
            .placeholder_vmo
            .get_or_insert_with(|| {
                Arc::new(Vmo::Wired(WiredVmo::new(Range::from(
                    PhysicalAddress::default()..PhysicalAddress::default(),
                ))))
            })
            .clone();

        let _region = self.regions.insert(AddressSpaceRegion::new(
            // self.mmu.clone(),
            range,
            permissions,
            vmo,
            0,
            name,
        ));

        log::debug!("TODO: reserve() materialize changes");

        // let mut mmu_aspace = self.mmu.lock();
        // let mut frame_alloc = FRAME_ALLOC.get().unwrap().lock();
        //
        // if flags.is_empty() {
        //     log::trace!("calling mmu_aspace.unmap({range:?}, {flags:?})");
        //     mmu_aspace.unmap(
        //         frame_alloc.deref_mut(),
        //         range.start,
        //         NonZeroUsize::new(range.size()).unwrap(),
        //         flush,
        //     )?;
        // } else {
        //     mmu_aspace.protect(
        //         range.start,
        //         NonZeroUsize::new(range.size()).unwrap(),
        //         flags,
        //         flush,
        //     )?;
        // }

        Ok(())
    }

    pub fn unmap(&mut self) {
        todo!()
    }

    pub fn page_fault(&mut self, virt: VirtualAddress, flags: PageFaultFlags) -> crate::Result<()> {
        assert!(flags.is_valid());

        let virt = virt.align_down(PAGE_SIZE);

        if let Some(region) = self.find_region(virt) {
            // TODO actually update self.last_fault here
            region.page_fault(virt, flags)
        } else {
            log::trace!("page fault at unmapped address {virt}");
            Err(Error::AccessDenied)
        }
    }

    fn find_region(&mut self, addr: VirtualAddress) -> Option<Pin<&mut AddressSpaceRegion>> {
        let region = self
            .regions
            .upper_bound_mut(Bound::Included(&addr))
            .get_mut()?;
        region.range.contains(&addr).then_some(region)
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

        debug_assert!(!self.regions.is_empty());
        // // if the tree is empty, treat max_range as the gap
        // if self.regions.is_empty() {
        //     let aligned_gap = Range::from(self.max_range)
        //         .checked_align_in(layout.align())
        //         .unwrap();
        //     let spot_count = spots_in_range(layout, aligned_gap.clone());
        //     candidate_spot_count += spot_count;
        //     if target_index < spot_count {
        //         return Ok(aligned_gap
        //             .start
        //             .checked_add(target_index << layout.align().ilog2())
        //             .unwrap());
        //     }
        //     target_index -= spot_count;
        // }

        // see if there is a suitable gap between the start of the address space and the first mapping
        if let Some(root) = self.regions.root().get() {
            let aligned_gap = Range::from(self.max_range.start..root.max_range.start)
                .checked_align_in(layout.align())
                .unwrap();
            let spot_count = spots_in_range(layout, aligned_gap);
            candidate_spot_count += spot_count;
            if target_index < spot_count {
                log::trace!("found gap left of tree in {aligned_gap:?}");
                return Ok(aligned_gap
                    .start
                    .checked_add(target_index << layout.align().ilog2())
                    .unwrap());
            }
            target_index -= spot_count;
        }

        let mut maybe_node = self.regions.root().get();
        let mut already_visited = VirtualAddress::default();

        while let Some(node) = maybe_node {
            if node.max_gap >= layout.size() {
                if let Some(left) = node.links.left() {
                    let left = unsafe { left.as_ref() };

                    if left.max_gap >= layout.size() && left.max_range.end > already_visited {
                        maybe_node = Some(left);
                        continue;
                    }

                    let aligned_gap = Range::from(left.max_range.end..node.range.start)
                        .checked_align_in(layout.align())
                        .unwrap();

                    let spot_count = spots_in_range(layout, aligned_gap);

                    candidate_spot_count += spot_count;
                    if target_index < spot_count {
                        log::trace!("found gap in left subtree in {aligned_gap:?}");
                        return Ok(aligned_gap
                            .start
                            .checked_add(target_index << layout.align().ilog2())
                            .unwrap());
                    }
                    target_index -= spot_count;
                }

                if let Some(right) = node.links.right() {
                    let right = unsafe { right.as_ref() };

                    let aligned_gap = Range::from(node.range.end..right.max_range.start)
                        .checked_align_in(layout.align())
                        .unwrap();

                    let spot_count = spots_in_range(layout, aligned_gap);

                    candidate_spot_count += spot_count;
                    if target_index < spot_count {
                        log::trace!("found gap in right subtree in {aligned_gap:?}");
                        return Ok(aligned_gap
                            .start
                            .checked_add(target_index << layout.align().ilog2())
                            .unwrap());
                    }
                    target_index -= spot_count;

                    if right.max_gap >= layout.size() && right.max_range.end > already_visited {
                        maybe_node = Some(right);
                        continue;
                    }
                }
            }
            already_visited = node.max_range.end;
            maybe_node = node.links.parent().map(|ptr| unsafe { ptr.as_ref() });
        }

        // see if there is a suitable gap between the end of the last mapping and the end of the address space
        if let Some(root) = self.regions.root().get() {
            let aligned_gap = Range::from(root.max_range.end..self.max_range.end)
                .checked_align_in(layout.align())
                .unwrap();
            let spot_count = spots_in_range(layout, aligned_gap);
            candidate_spot_count += spot_count;
            if target_index < spot_count {
                log::trace!("found gap right of tree in {aligned_gap:?}");
                return Ok(aligned_gap
                    .start
                    .checked_add(target_index << layout.align().ilog2())
                    .unwrap());
            }
        }

        Err(candidate_spot_count)
    }
}

pub struct Batch {
    // mmu: Arc<Mutex<mmu::AddressSpace>>,
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
    pub fn new() -> Self {
        Self {
            range: Default::default(),
            flags: mmu::Flags::empty(),
            phys: vec![],
        }
    }

    pub fn append(
        &mut self,
        base: VirtualAddress,
        phys: (PhysicalAddress, usize),
        flags: mmu::Flags,
    ) -> crate::Result<()> {
        debug_assert!(
            phys.1 % PAGE_SIZE == 0,
            "physical address range must be multiple of page size"
        );

        log::trace!("appending {phys:?} at {base:?} with flags {flags:?}");
        if !self.can_append(base) || self.flags != flags {
            self.flush()?;
            self.flags = flags;
            self.range = Range::from(base..base.checked_add(phys.1).unwrap());
        } else {
            self.range.end = self.range.end.checked_add(phys.1).unwrap();
        }

        self.phys.push(phys);

        Ok(())
    }

    pub fn flush(&mut self) -> crate::Result<()> {
        log::trace!("flushing batch {:?} {:?}...", self.range, self.phys);
        if self.phys.is_empty() {
            return Ok(());
        }

        log::trace!(
            "materializing changes to MMU {:?} {:?} {:?}",
            self.range,
            self.phys,
            self.flags
        );
        self.phys.clear();
        // let mut mmu = self.mmu.lock();
        // let mut flush = Flush::empty(mmu.asid());
        // let iter = BatchFramesIter {
        //     iter: self.phys.drain(..),
        // };
        // mmu.map(self.range.start, iter, self.flags, &mut flush)?;
        // todo!();

        self.range = Range::from(self.range.end..self.range.end);

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
    alloc: &'static FrameAllocator,
}
impl mmu::frame_alloc::FramesIterator for BatchFramesIter<'_> {
    fn alloc_mut(&mut self) -> &mut dyn mmu::frame_alloc::FrameAllocator {
        todo!()
        // &mut self.alloc
    }
}
impl Iterator for BatchFramesIter<'_> {
    type Item = (PhysicalAddress, usize);

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}
