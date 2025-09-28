// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::alloc::Layout;
use core::fmt;
use core::num::NonZeroUsize;
use core::ops::{Bound, DerefMut, Range, RangeBounds, RangeInclusive};
use core::pin::Pin;
use core::ptr::NonNull;

use anyhow::{bail, ensure};
use kmem::{AddressRangeExt, PhysicalAddress, VirtualAddress};
use rand::Rng;
use rand::distr::Uniform;
use rand_chacha::ChaCha20Rng;

use crate::arch;
use crate::mem::address_space_region::AddressSpaceRegion;
use crate::mem::frame_alloc::FrameAllocator;
use crate::mem::{ArchAddressSpace, Flush, PageFaultFlags, Permissions};

// const VIRT_ALLOC_ENTROPY: u8 = u8::try_from((arch::VIRT_ADDR_BITS - arch::PAGE_SHIFT as u32) + 1).unwrap();
const VIRT_ALLOC_ENTROPY: u8 = 27;

#[derive(Debug, Clone, Copy)]
pub enum AddressSpaceKind {
    User,
    Kernel,
}

pub struct AddressSpace {
    kind: AddressSpaceKind,
    /// A binary search tree of regions that make up this address space.
    pub(super) regions: wavltree::WAVLTree<AddressSpaceRegion>,
    /// The maximum range this address space can encompass.
    ///
    /// This is used to check new mappings against and speed up page fault handling.
    max_range: RangeInclusive<VirtualAddress>,
    /// The pseudo-random number generator used for address space layout randomization or `None`
    /// if ASLR is disabled.
    rng: Option<ChaCha20Rng>,
    /// The hardware address space backing this "logical" address space that changes need to be
    /// materialized into in order to take effect.
    pub arch: arch::AddressSpace,
    pub frame_alloc: &'static FrameAllocator,
    last_fault: Option<(NonNull<AddressSpaceRegion>, VirtualAddress)>,
}
// Safety: the last_fault field makes the not-Send, but its only ever accessed behind a &mut Self
unsafe impl Send for AddressSpace {}
// Safety: the last_fault field makes the not-Send, but its only ever accessed behind a &mut Self
unsafe impl Sync for AddressSpace {}

impl fmt::Debug for AddressSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AddressSpace")
            .field_with("regions", |f| {
                let mut f = f.debug_list();
                for region in self.regions.iter() {
                    f.entry(&format_args!(
                        "{:<40?} {}..{} {}",
                        region.name, region.range.start, region.range.end, region.permissions
                    ));
                }
                f.finish()
            })
            .field("max_range", &self.max_range)
            .field("kind", &self.kind)
            .field("rng", &self.rng)
            .finish_non_exhaustive()
    }
}

impl AddressSpace {
    pub fn new_user(
        asid: u16,
        rng: Option<ChaCha20Rng>,
        frame_alloc: &'static FrameAllocator,
    ) -> crate::Result<Self> {
        let (arch, _) = arch::AddressSpace::new(asid, frame_alloc)?;

        #[allow(tail_expr_drop_order, reason = "")]
        Ok(Self {
            regions: wavltree::WAVLTree::default(),
            arch,
            max_range: arch::USER_ASPACE_RANGE,
            rng,
            kind: AddressSpaceKind::User,
            frame_alloc,
            last_fault: None,
        })
    }

    pub unsafe fn from_active_kernel(
        arch_aspace: arch::AddressSpace,
        rng: Option<ChaCha20Rng>,
        frame_alloc: &'static FrameAllocator,
    ) -> Self {
        #[allow(tail_expr_drop_order, reason = "")]
        Self {
            regions: wavltree::WAVLTree::default(),
            arch: arch_aspace,
            max_range: arch::KERNEL_ASPACE_RANGE,
            rng,
            kind: AddressSpaceKind::Kernel,
            frame_alloc,
            last_fault: None,
        }
    }

    pub fn kind(&self) -> AddressSpaceKind {
        self.kind
    }

    pub unsafe fn activate(&self) {
        // Safety: ensured by caller
        unsafe { self.arch.activate() }
    }

    pub fn map(
        &mut self,
        layout: Layout,
        permissions: Permissions,
        map: impl FnOnce(
            Range<VirtualAddress>,
            Permissions,
            &mut Batch,
        ) -> crate::Result<AddressSpaceRegion>,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        ensure!(layout.pad_to_align().size() % arch::PAGE_SIZE == 0,);
        ensure!(
            layout.pad_to_align().size()
                <= self
                    .max_range
                    .end()
                    .checked_sub_addr(*self.max_range.start())
                    .unwrap_or_default(),
        );
        ensure!(layout.align() <= self.frame_alloc.max_alignment(),);
        ensure!(permissions.is_valid());

        // Actually do the mapping now
        // Safety: we checked all invariants above
        unsafe { self.map_unchecked(layout, permissions, map) }
    }

    pub unsafe fn map_unchecked(
        &mut self,
        layout: Layout,
        permissions: Permissions,
        map: impl FnOnce(
            Range<VirtualAddress>,
            Permissions,
            &mut Batch,
        ) -> crate::Result<AddressSpaceRegion>,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        let layout = layout.pad_to_align();
        let base = self.find_spot(layout, VIRT_ALLOC_ENTROPY)?;
        let range = base..base
            .checked_add(layout.size())
            .expect("chosen memory range end overflows");

        self.map_internal(range, permissions, map)
    }

    pub fn map_specific(
        &mut self,
        range: Range<VirtualAddress>,
        permissions: Permissions,
        map: impl FnOnce(
            Range<VirtualAddress>,
            Permissions,
            &mut Batch,
        ) -> crate::Result<AddressSpaceRegion>,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        ensure!(range.start.is_aligned_to(arch::PAGE_SIZE),);
        ensure!(range.end.is_aligned_to(arch::PAGE_SIZE),);
        ensure!(
            range.size()
                <= self
                    .max_range
                    .end()
                    .checked_sub_addr(*self.max_range.start())
                    .unwrap_or_default(),
        );
        ensure!(permissions.is_valid());
        // ensure the entire address space range is free
        if let Some(prev) = self.regions.upper_bound(range.start_bound()).get() {
            ensure!(prev.range.end <= range.start);
        }

        // Actually do the mapping now
        // Safety: we checked all invariants above
        unsafe { self.map_specific_unchecked(range, permissions, map) }
    }

    pub unsafe fn map_specific_unchecked(
        &mut self,
        range: Range<VirtualAddress>,
        permissions: Permissions,
        map: impl FnOnce(
            Range<VirtualAddress>,
            Permissions,
            &mut Batch,
        ) -> crate::Result<AddressSpaceRegion>,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        self.map_internal(range, permissions, map)
    }

    pub fn unmap(&mut self, range: Range<VirtualAddress>) -> crate::Result<()> {
        ensure!(range.start.is_aligned_to(arch::PAGE_SIZE),);
        ensure!(range.end.is_aligned_to(arch::PAGE_SIZE),);
        ensure!(
            range.size()
                <= self
                    .max_range
                    .end()
                    .checked_sub_addr(*self.max_range.start())
                    .unwrap_or_default(),
        );

        // ensure the entire range is mapped and doesn't cover any holes
        // `for_each_region_in_range` covers the last half so we just need to check that the regions
        // aren't smaller than the requested range.
        // We do that by adding up their sizes checking that their total size is at least as large
        // as the requested range.
        let mut bytes_seen = 0;
        self.for_each_region_in_range(range.clone(), |region| {
            bytes_seen += region.range.size();
            Ok(())
        })?;
        ensure!(bytes_seen == range.size());

        // Actually do the unmapping now
        // Safety: we checked all invariant above
        unsafe { self.unmap_unchecked(range) }
    }

    pub unsafe fn unmap_unchecked(&mut self, range: Range<VirtualAddress>) -> crate::Result<()> {
        let mut bytes_remaining = range.size();
        let mut c = self.regions.find_mut(&range.start);
        while bytes_remaining > 0 {
            let mut region = c.remove().unwrap();
            let range = region.range.clone();
            Pin::as_mut(&mut region).unmap(range.clone())?;
            bytes_remaining -= range.size();
        }

        let mut flush = self.arch.new_flush();
        // Safety: caller has to ensure invariants are checked
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

    pub fn protect(
        &mut self,
        range: Range<VirtualAddress>,
        new_permissions: Permissions,
    ) -> crate::Result<()> {
        ensure!(range.start.is_aligned_to(arch::PAGE_SIZE),);
        ensure!(range.end.is_aligned_to(arch::PAGE_SIZE),);
        ensure!(
            range.size()
                <= self
                    .max_range
                    .end()
                    .checked_sub_addr(*self.max_range.start())
                    .unwrap_or_default(),
        );
        ensure!(new_permissions.is_valid());

        // ensure the entire range is mapped and doesn't cover any holes
        // `for_each_region_in_range` covers the last half so we just need to check that the regions
        // aren't smaller than the requested range.
        // We do that by adding up their sizes checking that their total size is at least as large
        // as the requested range.
        // Along the way we also check for each region that the new permissions are a subset of the
        // current ones.
        let mut bytes_seen = 0;
        self.for_each_region_in_range(range.clone(), |region| {
            bytes_seen += region.range.size();

            ensure!(region.permissions.contains(new_permissions),);

            Ok(())
        })?;
        ensure!(bytes_seen == range.size());

        // Actually do the permission changes now
        // Safety: we checked all invariant above
        unsafe { self.protect_unchecked(range, new_permissions) }
    }

    pub unsafe fn protect_unchecked(
        &mut self,
        range: Range<VirtualAddress>,
        new_permissions: Permissions,
    ) -> crate::Result<()> {
        let mut bytes_remaining = range.size();
        let mut c = self.regions.find_mut(&range.start);
        while bytes_remaining > 0 {
            let mut region = c.get_mut().unwrap();
            region.permissions = new_permissions;
            bytes_remaining -= range.size();
        }

        let mut flush = self.arch.new_flush();
        // Safety: caller has to ensure invariants are checked
        unsafe {
            self.arch.update_flags(
                range.start,
                NonZeroUsize::new(range.size()).unwrap(),
                new_permissions.into(),
                &mut flush,
            )?;
        }
        flush.flush()?;

        Ok(())
    }

    pub fn page_fault(&mut self, addr: VirtualAddress, flags: PageFaultFlags) -> crate::Result<()> {
        assert!(flags.is_valid(), "invalid page fault flags {flags:?}");

        // make sure addr is even a valid address for this address space
        match self.kind {
            AddressSpaceKind::User => ensure!(
                arch::is_user_address(addr),
                "kernel fault in user space addr={addr}"
            ),
            AddressSpaceKind::Kernel => ensure!(
                arch::is_kernel_address(addr),
                "user fault in kernel space addr={addr}"
            ),
        }
        ensure!(
            self.max_range.contains(&addr),
            "page fault at address outside of address space range"
        );

        let addr = addr.align_down(arch::PAGE_SIZE);

        let region = if let Some((mut last_region, last_addr)) = self.last_fault.take() {
            // Safety: we pinky-promise this is fine
            let last_region = unsafe { Pin::new_unchecked(last_region.as_mut()) };

            assert_ne!(addr, last_addr, "double fault");

            if last_region.range.contains(&addr) {
                Some(last_region)
            } else {
                self.regions
                    .upper_bound_mut(Bound::Included(&addr))
                    .get_mut()
                    .and_then(|region| region.range.contains(&addr).then_some(region))
            }
        } else {
            self.regions
                .upper_bound_mut(Bound::Included(&addr))
                .get_mut()
                .and_then(|region| region.range.contains(&addr).then_some(region))
        };

        if let Some(mut region) = region {
            let region_ptr = NonNull::from(region.deref_mut());

            let mut batch = Batch::new(&mut self.arch, self.frame_alloc);
            region.page_fault(&mut batch, addr, flags)?;
            batch.flush()?;

            self.last_fault = Some((region_ptr, addr));

            Ok(())
        } else {
            bail!("page fault at unmapped address {addr}");
        }
    }

    pub fn reserve(
        &mut self,
        range: Range<VirtualAddress>,
        permissions: Permissions,
        name: Option<String>,
        flush: &mut Flush,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        ensure!(range.start.is_aligned_to(arch::PAGE_SIZE),);
        ensure!(range.end.is_aligned_to(arch::PAGE_SIZE),);
        ensure!(
            range.size()
                <= self
                    .max_range
                    .end()
                    .checked_sub_addr(*self.max_range.start())
                    .unwrap_or_default(),
        );
        ensure!(permissions.is_valid());

        // ensure the entire address space range is free
        if let Some(prev) = self.regions.upper_bound(range.start_bound()).get() {
            ensure!(prev.range.end <= range.start);
        }

        let region = AddressSpaceRegion::new_wired(range.clone(), permissions, name);
        let region = self.regions.insert(Box::pin(region));

        // eagerly materialize any possible changes, we do this eagerly for the entire range here
        // since `reserve` will only be called for kernel memory setup by the loader. For which it is
        // critical that the MMUs and our "logical" view are in sync.
        if permissions.is_empty() {
            // Safety: we checked all invariants above
            unsafe {
                self.arch
                    .unmap(range.start, NonZeroUsize::new(range.size()).unwrap(), flush)?;
            }
        } else {
            // Safety: we checked all invariants above
            unsafe {
                self.arch.update_flags(
                    range.start,
                    NonZeroUsize::new(range.size()).unwrap(),
                    permissions.into(),
                    flush,
                )?;
            }
        }

        Ok(region)
    }

    pub fn commit(&mut self, range: Range<VirtualAddress>, will_write: bool) -> crate::Result<()> {
        ensure!(range.start.is_aligned_to(arch::PAGE_SIZE),);
        ensure!(range.end.is_aligned_to(arch::PAGE_SIZE),);
        ensure!(
            range.size()
                <= self
                    .max_range
                    .end()
                    .checked_sub_addr(*self.max_range.start())
                    .unwrap_or_default(),
        );

        let mut batch = Batch::new(&mut self.arch, self.frame_alloc);
        let mut bytes_remaining = range.size();
        let mut c = self.regions.find_mut(&range.start);
        while bytes_remaining > 0 {
            let region = c.get_mut().unwrap();
            let clamped = range.clamp(region.range.clone());
            region.commit(&mut batch, clamped, will_write)?;

            bytes_remaining -= range.size();
        }
        batch.flush()?;

        Ok(())
    }

    fn map_internal(
        &mut self,
        range: Range<VirtualAddress>,
        permissions: Permissions,
        map: impl FnOnce(
            Range<VirtualAddress>,
            Permissions,
            &mut Batch,
        ) -> crate::Result<AddressSpaceRegion>,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        let mut batch = Batch::new(&mut self.arch, self.frame_alloc);
        let region = map(range, permissions, &mut batch)?;
        let region = self.regions.insert(Box::pin(region));

        // TODO eagerly map a few pages now

        batch.flush()?;
        Ok(region)
    }

    /// Calls the provided callback for each `AddressSpaceRegion` in the given virtual address range.
    /// This method will ensure the provided range does not cover any holes where no region exists,
    /// returning an error on the first hole encountered.
    fn for_each_region_in_range<F>(
        &self,
        range: Range<VirtualAddress>,
        mut f: F,
    ) -> crate::Result<()>
    where
        F: FnMut(&AddressSpaceRegion) -> crate::Result<()>,
    {
        let mut prev_end = None;
        for region in self.regions.range(range) {
            // ensure there is no gap between this region and the previous one
            if let Some(prev_end) = prev_end.replace(region.range.end)
                && prev_end != region.range.start
            {
                bail!("not mapped");
            }

            // call the callback
            f(region)?;
        }

        Ok(())
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
    fn find_spot(&mut self, layout: Layout, entropy: u8) -> crate::Result<VirtualAddress> {
        // behaviour:
        // - find the leftmost gap that satisfies the size and alignment requirements
        //      - starting at the root,
        // tracing::trace!("finding spot for {layout:?} entropy {entropy}");

        let max_candidate_spaces: usize = 1 << entropy;
        // tracing::trace!("max_candidate_spaces {max_candidate_spaces}");

        let selected_index: usize = self
            .rng
            .as_mut()
            .map(|prng| prng.sample(Uniform::new(0, max_candidate_spaces).unwrap()))
            .unwrap_or_default();

        let spot = match self.find_spot_at_index(selected_index, layout) {
            Ok(spot) => spot,
            Err(0) => bail!("out of virtual memory"),
            Err(candidate_spot_count) => {
                // tracing::trace!("couldn't find spot in first attempt (max_candidate_spaces {max_candidate_spaces}), retrying with (candidate_spot_count {candidate_spot_count})");
                let selected_index: usize = self
                    .rng
                    .as_mut()
                    .unwrap()
                    .sample(Uniform::new(0, candidate_spot_count).unwrap());

                self.find_spot_at_index(selected_index, layout).unwrap()
            }
        };
        tracing::trace!("picked spot {spot}..{:?}", spot.checked_add(layout.size()));

        debug_assert!(arch::is_canonical(spot));
        Ok(spot)
    }

    #[expect(clippy::undocumented_unsafe_blocks, reason = "intrusive tree access")]
    fn find_spot_at_index(
        &self,
        mut target_index: usize,
        layout: Layout,
    ) -> Result<VirtualAddress, usize> {
        tracing::trace!("attempting to find spot for {layout:?} at index {target_index}");

        let spots_in_range = |layout: Layout, aligned: Range<VirtualAddress>| -> usize {
            debug_assert!(
                aligned.start.is_aligned_to(layout.align())
                    && aligned.end.is_aligned_to(layout.align())
            );

            // ranges passed in here can become empty for a number of reasons (aligning might produce ranges
            // where end > start, or the range might be empty to begin with) in either case an empty
            // range means no spots are available
            if aligned.is_empty() {
                return 0;
            }

            let range_size = aligned.size();
            if range_size >= layout.size() {
                ((range_size - layout.size()) >> layout.align().ilog2()) + 1
            } else {
                0
            }
        };

        let mut candidate_spot_count = 0;

        // if the tree is empty, treat max_range as the gap
        if self.regions.is_empty() {
            let aligned_gap = Range {
                start: self
                    .max_range
                    .start()
                    .checked_align_up(layout.align())
                    .unwrap(),
                end: self
                    .max_range
                    .end()
                    .checked_sub(1)
                    .unwrap()
                    .align_down(layout.align()),
            };

            let spot_count = spots_in_range(layout, aligned_gap.clone());
            candidate_spot_count += spot_count;
            if target_index < spot_count {
                tracing::trace!("tree is empty, chose gap {aligned_gap:?}");
                return Ok(aligned_gap
                    .start
                    .checked_add(target_index << layout.align().ilog2())
                    .unwrap());
            }
            target_index -= spot_count;
        }

        // see if there is a suitable gap between the start of the address space and the first mapping
        if let Some(root) = self.regions.root().get() {
            let aligned_gap = (*self.max_range.start()..root.max_range.start)
                .checked_align_in(layout.align())
                .unwrap();
            let spot_count = spots_in_range(layout, aligned_gap.clone());
            candidate_spot_count += spot_count;
            if target_index < spot_count {
                tracing::trace!("found gap left of tree in {aligned_gap:?}");
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

                    let aligned_gap = (left.max_range.end..node.range.start)
                        .checked_align_in(layout.align())
                        .unwrap();

                    let spot_count = spots_in_range(layout, aligned_gap.clone());

                    candidate_spot_count += spot_count;
                    if target_index < spot_count {
                        tracing::trace!("found gap in left subtree in {aligned_gap:?}");
                        return Ok(aligned_gap
                            .start
                            .checked_add(target_index << layout.align().ilog2())
                            .unwrap());
                    }
                    target_index -= spot_count;
                }

                if let Some(right) = node.links.right() {
                    let right = unsafe { right.as_ref() };

                    let aligned_gap = (node.range.end..right.max_range.start)
                        .checked_align_in(layout.align())
                        .unwrap();

                    let spot_count = spots_in_range(layout, aligned_gap.clone());

                    candidate_spot_count += spot_count;
                    if target_index < spot_count {
                        tracing::trace!("found gap in right subtree in {aligned_gap:?}");
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
            let aligned_gap = (root.max_range.end..*self.max_range.end())
                .checked_align_in(layout.align())
                .unwrap();
            let spot_count = spots_in_range(layout, aligned_gap.clone());
            candidate_spot_count += spot_count;
            if target_index < spot_count {
                tracing::trace!("found gap right of tree in {aligned_gap:?}");
                return Ok(aligned_gap
                    .start
                    .checked_add(target_index << layout.align().ilog2())
                    .unwrap());
            }
        }

        Err(candidate_spot_count)
    }
}

// === Batch ===

pub struct Batch<'a> {
    pub aspace: &'a mut arch::AddressSpace,
    pub frame_alloc: &'static FrameAllocator,
    range: Range<VirtualAddress>,
    flags: <arch::AddressSpace as ArchAddressSpace>::Flags,
    actions: Vec<BBatchAction>,
}

#[derive(Debug)]
enum BBatchAction {
    Map(PhysicalAddress, usize),
}

impl Drop for Batch<'_> {
    fn drop(&mut self) {
        if !self.actions.is_empty() {
            tracing::error!("batch was not flushed before dropping");
            // panic_unwind::panic_in_drop!("batch was not flushed before dropping");
        }
    }
}

impl<'a> Batch<'a> {
    pub fn new(aspace: &'a mut arch::AddressSpace, frame_alloc: &'static FrameAllocator) -> Self {
        Self {
            aspace,
            frame_alloc,
            range: Range::default(),
            flags: <arch::AddressSpace as ArchAddressSpace>::Flags::empty(),
            actions: vec![],
        }
    }

    pub fn queue_map(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        len: NonZeroUsize,
        flags: <arch::AddressSpace as ArchAddressSpace>::Flags,
    ) -> crate::Result<()> {
        debug_assert!(
            len.get().is_multiple_of(arch::PAGE_SIZE),
            "physical address range must be multiple of page size"
        );

        tracing::trace!("appending {phys:?} at {virt:?} with flags {flags:?}");
        if self.range.end != virt || self.flags != flags {
            self.flush()?;
            self.flags = flags;
            self.range = virt..virt.checked_add(len.get()).unwrap();
        } else {
            self.range.end = self.range.end.checked_add(len.get()).unwrap();
        }

        self.actions.push(BBatchAction::Map(phys, len.get()));

        Ok(())
    }

    pub fn flush(&mut self) -> crate::Result<()> {
        if self.actions.is_empty() {
            return Ok(());
        }
        tracing::trace!("flushing batch {:?} {:?}...", self.range, self.actions);

        let mut flush = self.aspace.new_flush();
        let mut virt = self.range.start;
        for action in self.actions.drain(..) {
            match action {
                // Safety: we have checked all the invariants
                BBatchAction::Map(phys, len) => unsafe {
                    self.aspace.map_contiguous(
                        self.frame_alloc,
                        virt,
                        phys,
                        NonZeroUsize::new(len).unwrap(),
                        self.flags,
                        &mut flush,
                    )?;
                    virt = virt.checked_add(len).unwrap();
                },
            }
        }
        flush.flush()?;

        self.range = self.range.end..self.range.end;
        Ok(())
    }

    pub fn ignore(&mut self) {
        self.actions.clear();
    }
}
