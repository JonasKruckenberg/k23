// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::error::Error;
use crate::vm::address::{AddressRangeExt, PhysicalAddress, VirtualAddress};
use crate::vm::address_space_region::AddressSpaceRegion;
use crate::vm::flush::Flush;
use crate::vm::vmo::{Vmo, WiredVmo};
use crate::vm::ArchAddressSpace;
use crate::vm::{PageFaultFlags, Permissions};
use crate::{arch, ensure};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::alloc::Layout;
use core::cmp;
use core::num::NonZeroUsize;
use core::ops::Bound;
use core::pin::Pin;
use core::range::{Range, RangeBounds};
use rand::distributions::Uniform;
use rand::Rng;
use rand_chacha::ChaCha20Rng;

// const VIRT_ALLOC_ENTROPY: u8 = u8::try_from((arch::VIRT_ADDR_BITS - arch::PAGE_SHIFT as u32) + 1).unwrap();
const VIRT_ALLOC_ENTROPY: u8 = 27;

pub enum AddressSpaceKind {
    User,
    Kernel,
}

/// Represents the address space of a process (or the kernel).
pub struct AddressSpace {
    /// A binary search tree of regions that make up this address space.
    pub(crate) regions: wavltree::WAVLTree<AddressSpaceRegion>,
    /// The hardware address space backing this "logical" address space that changes need to be
    /// materialized into in order to take effect.
    arch: arch::AddressSpace,
    /// The maximum range this address space can encompass.
    ///
    /// This is used to check new mappings against and speed up page fault handling.
    max_range: Range<VirtualAddress>,
    /// The pseudo-random number generator used for address space layout randomization or `None`
    /// if ASLR is disabled.
    prng: Option<ChaCha20Rng>,
    /// "Empty" placeholder VMO to back regions created by `reserve`
    placeholder_vmo: Option<Arc<Vmo>>,
    kind: AddressSpaceKind,
}

impl AddressSpace {
    pub fn new_user(arch_aspace: arch::AddressSpace, prng: Option<ChaCha20Rng>) -> Self {
        #![allow(tail_expr_drop_order)]
        Self {
            regions: wavltree::WAVLTree::default(),
            arch: arch_aspace,
            max_range: Range::from(arch::USER_ASPACE_BASE..VirtualAddress::MAX),
            prng,
            placeholder_vmo: None,
            kind: AddressSpaceKind::User,
        }
    }

    pub fn from_active_kernel(arch_aspace: arch::AddressSpace, prng: Option<ChaCha20Rng>) -> Self {
        #![allow(tail_expr_drop_order)]
        Self {
            regions: wavltree::WAVLTree::default(),
            arch: arch_aspace,
            max_range: Range::from(arch::KERNEL_ASPACE_BASE..VirtualAddress::MAX),
            prng,
            placeholder_vmo: None,
            kind: AddressSpaceKind::Kernel,
        }
    }

    /// Crate a new region in this address space.
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
    pub fn map(
        &mut self,
        layout: Layout,
        vmo: Arc<Vmo>,
        vmo_offset: usize,
        permissions: Permissions,
        name: Option<String>,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        let base = self.find_spot(layout, VIRT_ALLOC_ENTROPY);
        let virt = Range::from(base..base.checked_add(layout.size()).unwrap());

        let region = self.regions.insert(AddressSpaceRegion::new(
            virt,
            permissions,
            vmo,
            vmo_offset,
            name,
        ));
        // mapping.map_range(batch, virt)?;

        Ok(region)
    }

    /// Create a new region at the provided range in this address space.
    ///
    /// It's memory will be backed by the provided `vmo` at the given `vmo_offset`.
    pub fn map_specific(
        &mut self,
        virt: Range<VirtualAddress>,
        vmo: Arc<Vmo>,
        vmo_offset: usize,
        permissions: Permissions,
        name: Option<String>,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        assert!(virt.start.is_aligned_to(arch::PAGE_SIZE));
        assert!(virt.end.is_aligned_to(arch::PAGE_SIZE));
        assert_eq!(vmo_offset % arch::PAGE_SIZE, 0);

        if let Some(prev) = self.regions.upper_bound(virt.start_bound()).get() {
            assert!(prev.range.end <= virt.start);
        }

        // TODO can we reuse the cursor we previously created for this?
        let region = self.regions.insert(AddressSpaceRegion::new(
            virt,
            permissions,
            vmo,
            vmo_offset,
            name,
        ));
        // mapping.map_range(batch, virt)?;

        Ok(region)
    }

    pub fn reserve(
        &mut self,
        range: Range<VirtualAddress>,
        permissions: Permissions,
        name: Option<String>,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        log::trace!("reserving {range:?} with flags {permissions:?} and name {name:?}");

        let vmo = self
            .placeholder_vmo
            .get_or_insert_with(|| {
                Arc::new(Vmo::Wired(WiredVmo::new(Range::from(
                    PhysicalAddress::default()..PhysicalAddress::default(),
                ))))
            })
            .clone();

        let _region =
            self.regions
                .insert(AddressSpaceRegion::new(range, permissions, vmo, 0, name));

        if permissions.is_empty() {
            log::trace!("calling mmu_aspace.unmap({range:?})",);
            unsafe {
                self.arch
                    .unmap(range.start, NonZeroUsize::new(range.size()).unwrap(), flush)?;
            }
        } else {
            unsafe {
                self.arch.protect(
                    range.start,
                    NonZeroUsize::new(range.size()).unwrap(),
                    permissions.into(),
                    flush,
                )?;
            }
        }

        Ok(())
    }

    pub fn unmap(&mut self, range: Range<VirtualAddress>) -> crate::Result<()> {
        assert!(range.start.is_aligned_to(arch::PAGE_SIZE));
        let range = range.checked_align_out(arch::PAGE_SIZE).unwrap();
        let mut iter = self.regions.range_mut(range);

        while let Some(mut region) = iter.next() {
            log::trace!("{region:?}");
            let start = cmp::max(region.range.start, range.start);
            let end = cmp::min(region.range.end, range.end);

            if range.start <= region.range.start && range.end >= region.range.end {
                // this mappings range is entirely contained within `range`, so we need
                // fully remove the mapping from the tree
                log::trace!("TODO remove region");
            } else if range.start > region.range.start && range.end < region.range.end {
                // `range` is entirely contained within the mappings range, we
                // need to split the range in two
                log::trace!("splitting region");
                let right = Range::from(range.end..region.range.end);
                let right_vmo_offset = region.range.end.checked_sub_addr(range.end).unwrap();
                log::trace!("right_vmo_offset {right_vmo_offset}");

                iter.tree().insert(AddressSpaceRegion::new(
                    right,
                    region.permissions,
                    region.vmo.clone(),
                    right_vmo_offset,
                    region.name.clone(),
                ));

                region.range.end = range.start;
            } else if range.start > region.range.start {
                // `range` is mostly past this mappings range, but overlaps partially
                // we need adjust the ranges end
                log::trace!("adjusting region end");
                region.range.end = range.start;
            } else if range.end < region.range.end {
                // `range` is mostly before this mappings range, but overlaps partially
                // we need adjust the ranges start
                log::trace!("adjusting region start");
                region.range.start = range.end;
            } else {
                unreachable!()
            }

            region.unmap(Range::from(start..end))?;
        }

        let mut flush = self.arch.new_flush();
        unsafe {
            self.arch.unmap(
                range.start,
                NonZeroUsize::new(range.size()).unwrap(),
                &mut flush,
            )?;
        }
        flush.flush()?;

        Ok(())
    }

    /// Page fault handling
    ///
    /// - Is there an `AddressSpaceRegion` for the address?
    ///     - NO => Error::AccessDenied (fault at unmapped address)
    ///     - YES
    ///         - ensure access is coherent with logical permissions
    ///         - Is there a `Frame` for the address?
    ///             - NO  => TODO this means frame got paged-out, need to reload it from pager
    ///             - YES (frame is resident in memory, but flags were off, either a fluke or COW)
    ///                 - Is access WRITE?
    ///                     - NO  => (do nothing)
    ///                     - YES => Need to do COW
    ///                         - allocate new frame
    ///                         - copy content from old to new frame (OMIT FOR ZERO FRAME)
    ///                         - replace old frame with new frame
    ///                      - update MMU page table
    pub fn page_fault(&mut self, addr: VirtualAddress, flags: PageFaultFlags) -> crate::Result<()> {
        assert!(flags.is_valid());

        // make sure addr is even a valid address for this address space
        match self.kind {
            AddressSpaceKind::User => ensure!(
                addr.is_user_accessible(),
                Error::AccessDenied,
                "non-user address fault in user address space"
            ),
            AddressSpaceKind::Kernel => ensure!(
                arch::is_kernel_address(addr),
                Error::AccessDenied,
                "non-kernel address fault in kernel address space"
            ),
        }
        ensure!(
            self.max_range.contains(&addr),
            Error::AccessDenied,
            "non-kernel address fault in kernel address space"
        );

        let addr = addr.align_down(arch::PAGE_SIZE);

        let region = self
            .regions
            .upper_bound_mut(Bound::Included(&addr))
            .get_mut()
            .and_then(|region| region.range.contains(&addr).then_some(region));

        if let Some(region) = region {
            let mut batch = Batch::new(&mut self.arch);
            region.page_fault(&mut batch, addr, flags)?;
            batch.flush()?;
            Ok(())
        } else {
            log::trace!("page fault at unmapped address {addr}");
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

// =============================================================================
// Batch
// =============================================================================

pub struct Batch<'a> {
    arch_aspace: &'a mut arch::AddressSpace,
    range: Range<VirtualAddress>,
    flags: <arch::AddressSpace as ArchAddressSpace>::Flags,
    phys: Vec<(PhysicalAddress, usize)>,
}

impl Drop for Batch<'_> {
    fn drop(&mut self) {
        if !self.phys.is_empty() {
            log::error!("batch was not flushed before dropping");
            // panic_unwind::panic_in_drop!("batch was not flushed before dropping");
        }
    }
}

impl<'a> Batch<'a> {
    pub fn new(arch_aspace: &'a mut arch::AddressSpace) -> Self {
        Self {
            arch_aspace,
            range: Default::default(),
            flags: <arch::AddressSpace as ArchAddressSpace>::Flags::empty(),
            phys: vec![],
        }
    }

    pub fn append(
        &mut self,
        base: VirtualAddress,
        phys: PhysicalAddress,
        len: usize,
        flags: <arch::AddressSpace as ArchAddressSpace>::Flags,
    ) -> crate::Result<()> {
        debug_assert!(
            len % arch::PAGE_SIZE == 0,
            "physical address range must be multiple of page size"
        );

        log::trace!("appending {phys:?} at {base:?} with flags {flags:?}");
        if !self.can_append(base) || self.flags != flags {
            self.flush()?;
            self.flags = flags;
            self.range = Range::from(base..base.checked_add(len).unwrap());
        } else {
            self.range.end = self.range.end.checked_add(len).unwrap();
        }

        self.phys.push((phys, len));

        Ok(())
    }

    pub fn flush(&mut self) -> crate::Result<()> {
        if self.phys.is_empty() {
            return Ok(());
        }
        log::trace!("flushing batch {:?} {:?}...", self.range, self.phys);

        let mut flush = self.arch_aspace.new_flush();
        for (phys, len) in self.phys.drain(..) {
            unsafe {
                self.arch_aspace.map_contiguous(
                    self.range.start,
                    phys,
                    NonZeroUsize::new(len).unwrap(),
                    self.flags,
                    &mut flush,
                )?;
            }
        }
        flush.flush()?;

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
