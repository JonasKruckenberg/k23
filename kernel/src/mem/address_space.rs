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
use core::ops::{Bound, DerefMut, Range, RangeBounds};
use core::pin::Pin;
use core::ptr::NonNull;

use anyhow::{anyhow, bail, ensure};
use kmem_core::{
    AddressRangeExt, Arch, Flush, FrameAllocator as _, MemoryAttributes, PhysicalAddress,
    VirtualAddress,
};
use rand_chacha::ChaCha20Rng;

use crate::arch;
use crate::mem::address_space_region::AddressSpaceRegion;
use crate::mem::frame_alloc::FrameAllocator;
use crate::mem::PageFaultFlags;

#[derive(Debug, Clone, Copy)]
pub enum AddressSpaceKind {
    User,
    Kernel,
}

pub struct AddressSpace<A: Arch> {
    kind: AddressSpaceKind,
    /// A binary search tree of regions that make up this address space.
    pub(super) regions: wavltree::WAVLTree<AddressSpaceRegion>,
    /// The pseudo-random number generator used for address space layout randomization or `None`
    /// if ASLR is disabled.
    rng: Option<ChaCha20Rng>,
    /// The hardware address space backing this "logical" address space that changes need to be
    /// materialized into in order to take effect.
    pub raw: kmem_core::AddressSpace<A>,
    pub frame_alloc: &'static FrameAllocator,
    last_fault: Option<(NonNull<AddressSpaceRegion>, VirtualAddress)>,
}
// Safety: the last_fault field makes the not-Send, but its only ever accessed behind a &mut Self
unsafe impl<A> Send for AddressSpace<A> where A: Arch + Send {}
// Safety: the last_fault field makes the not-Send, but its only ever accessed behind a &mut Self
unsafe impl<A> Sync for AddressSpace<A> where A: Arch + Sync {}

impl<A> fmt::Debug for AddressSpace<A>
where
    A: Arch + fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AddressSpace")
            .field("raw", &self.raw)
            .field_with("regions", |f| {
                let mut f = f.debug_list();
                for region in self.regions.iter() {
                    f.entry(&format_args!(
                        "{:<40?} {}..{} {}",
                        region.name, region.range.start, region.range.end, region.attributes
                    ));
                }
                f.finish()
            })
            .field("kind", &self.kind)
            .field("rng", &self.rng)
            .finish_non_exhaustive()
    }
}

impl<A: Arch> AddressSpace<A> {
    pub unsafe fn new(
        raw_aspace: kmem_core::AddressSpace<A>,
        rng: Option<ChaCha20Rng>,
        frame_alloc: &'static FrameAllocator,
    ) -> Self {
        #[allow(tail_expr_drop_order, reason = "")]
        Self {
            regions: wavltree::WAVLTree::default(),
            raw: raw_aspace,
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
        unsafe { self.raw.activate() }
    }

    pub fn map(
        &mut self,
        layout: Layout,
        attributes: MemoryAttributes,
        map: impl FnOnce(
            Range<VirtualAddress>,
            MemoryAttributes,
            &mut Batch<A>,
        ) -> crate::Result<AddressSpaceRegion>,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        ensure!(layout.pad_to_align().size() % arch::PAGE_SIZE == 0);
        ensure!(Some(NonZeroUsize::new(layout.align()).unwrap()) <= self.frame_alloc.size_hint().1);

        // Actually do the mapping now
        // Safety: we checked all invariants above
        unsafe { self.map_unchecked(layout, attributes, map) }
    }

    pub unsafe fn map_unchecked(
        &mut self,
        layout: Layout,
        attributes: MemoryAttributes,
        map: impl FnOnce(
            Range<VirtualAddress>,
            MemoryAttributes,
            &mut Batch<A>,
        ) -> crate::Result<AddressSpaceRegion>,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        let layout = layout.pad_to_align();
        let base = self
            .find_spot_for(layout)
            .ok_or(anyhow!("out of virtual memory"))?;
        let range = Range::from_start_len(base, layout.size());

        self.map_internal(range, attributes, map)
    }

    pub fn map_specific(
        &mut self,
        range: Range<VirtualAddress>,
        attributes: MemoryAttributes,
        map: impl FnOnce(
            Range<VirtualAddress>,
            MemoryAttributes,
            &mut Batch<A>,
        ) -> crate::Result<AddressSpaceRegion>,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        ensure!(range.start.is_aligned_to(arch::PAGE_SIZE),);
        ensure!(range.end.is_aligned_to(arch::PAGE_SIZE),);

        // ensure the entire address space range is free
        if let Some(prev) = self.regions.upper_bound(range.start_bound()).get() {
            ensure!(prev.range.end <= range.start);
        }

        // Actually do the mapping now
        // Safety: we checked all invariants above
        unsafe { self.map_specific_unchecked(range, attributes, map) }
    }

    pub unsafe fn map_specific_unchecked(
        &mut self,
        range: Range<VirtualAddress>,
        attributes: MemoryAttributes,
        map: impl FnOnce(
            Range<VirtualAddress>,
            MemoryAttributes,
            &mut Batch<A>,
        ) -> crate::Result<AddressSpaceRegion>,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        self.map_internal(range, attributes, map)
    }

    pub fn unmap(&mut self, range: Range<VirtualAddress>) -> crate::Result<()> {
        ensure!(range.start.is_aligned_to(arch::PAGE_SIZE),);
        ensure!(range.end.is_aligned_to(arch::PAGE_SIZE),);

        // ensure the entire range is mapped and doesn't cover any holes
        // `for_each_region_in_range` covers the last half so we just need to check that the regions
        // aren't smaller than the requested range.
        // We do that by adding up their sizes checking that their total size is at least as large
        // as the requested range.
        let mut bytes_seen = 0;
        self.for_each_region_in_range(range.clone(), |region| {
            bytes_seen += region.range.len();
            Ok(())
        })?;
        ensure!(bytes_seen == range.len());

        // Actually do the unmapping now
        // Safety: we checked all invariant above
        unsafe { self.unmap_unchecked(range) }
    }

    pub unsafe fn unmap_unchecked(&mut self, range: Range<VirtualAddress>) -> crate::Result<()> {
        let mut bytes_remaining = range.len();
        let mut c = self.regions.find_mut(&range.start);
        while bytes_remaining > 0 {
            let mut region = c.remove().unwrap();
            let range = region.range.clone();
            Pin::as_mut(&mut region).unmap(range.clone())?;
            bytes_remaining -= range.len();
        }

        let mut flush = Flush::new();
        // Safety: caller has to ensure invariants are checked
        unsafe {
            self.raw.unmap(range, self.frame_alloc, &mut flush);
        }
        flush.flush(self.raw.arch());

        Ok(())
    }

    pub fn protect(
        &mut self,
        range: Range<VirtualAddress>,
        new_attributes: MemoryAttributes,
    ) -> crate::Result<()> {
        ensure!(range.start.is_aligned_to(arch::PAGE_SIZE));
        ensure!(range.end.is_aligned_to(arch::PAGE_SIZE));

        // ensure the entire range is mapped and doesn't cover any holes
        // `for_each_region_in_range` covers the last half so we just need to check that the regions
        // aren't smaller than the requested range.
        // We do that by adding up their sizes checking that their total size is at least as large
        // as the requested range.
        // Along the way we also check for each region that the new permissions are a subset of the
        // current ones.
        let mut bytes_seen = 0;
        self.for_each_region_in_range(range.clone(), |region| {
            bytes_seen += region.range.len();

            ensure!(region.attributes.contains(new_attributes),);

            Ok(())
        })?;
        ensure!(bytes_seen == range.len());

        // Actually do the permission changes now
        // Safety: we checked all invariant above
        unsafe { self.protect_unchecked(range, new_attributes) }
    }

    pub unsafe fn protect_unchecked(
        &mut self,
        range: Range<VirtualAddress>,
        new_attributes: MemoryAttributes,
    ) -> crate::Result<()> {
        let mut bytes_remaining = range.len();
        let mut c = self.regions.find_mut(&range.start);
        while bytes_remaining > 0 {
            let mut region = c.get_mut().unwrap();
            region.attributes = new_attributes;
            bytes_remaining -= range.len();
        }

        let mut flush = Flush::new();
        // Safety: caller has to ensure invariants are checked
        unsafe {
            self.raw.set_attributes(range, new_attributes, &mut flush);
        }
        flush.flush(self.raw.arch());

        Ok(())
    }

    pub fn page_fault(&mut self, addr: VirtualAddress, flags: PageFaultFlags) -> crate::Result<()> {
        assert!(flags.is_valid(), "invalid page fault flags {flags:?}");

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

            let mut batch = Batch::new(&mut self.raw, self.frame_alloc);
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
        attributes: MemoryAttributes,
        name: Option<String>,
        flush: &mut Flush,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        ensure!(range.start.is_aligned_to(arch::PAGE_SIZE));
        ensure!(range.end.is_aligned_to(arch::PAGE_SIZE));

        // ensure the entire address space range is free
        if let Some(prev) = self.regions.upper_bound(range.start_bound()).get() {
            ensure!(prev.range.end <= range.start);
        }

        let region = AddressSpaceRegion::new_wired(range.clone(), attributes, name);
        let region = self.regions.insert(Box::pin(region));

        // eagerly materialize any possible changes, we do this eagerly for the entire range here
        // since `reserve` will only be called for kernel memory setup by the loader. For which it is
        // critical that the MMUs and our "logical" view are in sync.
        if attributes.bits() == 0 {
            // Safety: we checked all invariants above
            unsafe {
                self.raw.unmap(range, self.frame_alloc, flush);
            }
        } else {
            // Safety: we checked all invariants above
            unsafe {
                self.raw.set_attributes(range, attributes, flush);
            }
        }

        Ok(region)
    }

    pub fn commit(&mut self, range: Range<VirtualAddress>, will_write: bool) -> crate::Result<()> {
        ensure!(range.start.is_aligned_to(arch::PAGE_SIZE));
        ensure!(range.end.is_aligned_to(arch::PAGE_SIZE));

        let mut batch = Batch::new(&mut self.raw, self.frame_alloc);
        let mut bytes_remaining = range.len();
        let mut c = self.regions.find_mut(&range.start);
        while bytes_remaining > 0 {
            let region = c.get_mut().unwrap();
            let clamped = range.clone().intersect(region.range.clone());
            region.commit(&mut batch, clamped, will_write)?;

            bytes_remaining -= range.len();
        }
        batch.flush()?;

        Ok(())
    }

    fn map_internal(
        &mut self,
        range: Range<VirtualAddress>,
        attributes: MemoryAttributes,
        map: impl FnOnce(
            Range<VirtualAddress>,
            MemoryAttributes,
            &mut Batch<A>,
        ) -> crate::Result<AddressSpaceRegion>,
    ) -> crate::Result<Pin<&mut AddressSpaceRegion>> {
        let mut batch = Batch::new(&mut self.raw, self.frame_alloc);
        let region = map(range, attributes, &mut batch)?;
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

    fn find_spot_for(&mut self, layout: Layout) -> Option<VirtualAddress> {
        let mut gaps = Gaps {
            layout,
            stack: vec![],
            prev_region_end: Some(VirtualAddress::MIN),
            max_range_end: VirtualAddress::MAX,
        };
        gaps.push_left_nodes(self.regions.root().get().unwrap());

        kmem_aslr::find_spot_for(
            layout,
            gaps,
            self.raw.arch().memory_mode().virtual_address_bits()
                - self.raw.arch().memory_mode().page_size().ilog2() as u8,
            self.rng.as_mut(),
        )
    }
}

#[derive(Debug, Clone)]
struct Gaps<'a> {
    layout: Layout,
    stack: Vec<&'a AddressSpaceRegion>,
    prev_region_end: Option<VirtualAddress>,
    max_range_end: VirtualAddress,
}

impl<'a> Gaps<'a> {
    fn push_left_nodes(&mut self, mut node: &'a AddressSpaceRegion) {
        // while node.suitable_gap_in_subtree(self.layout) {
        loop {
            self.stack.push(node);
            if let Some(left) = node.left_child() {
                node = left;
            } else {
                break;
            }
        }
    }
}

impl Iterator for Gaps<'_> {
    type Item = Range<VirtualAddress>;

    fn next(&mut self) -> Option<Self::Item> {
        let prev_region_end = self.prev_region_end.take()?;

        while let Some(node) = self.stack.pop() {
            // compute gap size (use VirtualAddress subtraction helper)
            let gap_size = node.range.start.offset_from_unsigned(prev_region_end);
            // if the gap is large enough yield it
            if gap_size >= self.layout.size() {
                // prepare next traversal: push right subtree if it can contain suitable gaps
                if let Some(right) = node.right_child() {
                    // if right.suitable_gap_in_subtree(self.layout) {
                    self.push_left_nodes(right);
                    // }
                }

                let gap = prev_region_end..node.range.start;

                // update prev_end to current node end before yielding
                self.prev_region_end = Some(node.range.end);

                return Some(gap);
            }

            // no gap yielded for this node, continue traversal: push right subtree if interesting
            if let Some(right) = node.right_child() {
                if right.suitable_gap_in_subtree(self.layout) {
                    self.push_left_nodes(right);
                }
            } else {
                // ensure prev_end reflects the most-recent visited node end
                self.prev_region_end = Some(node.range.end);
            }
        }

        Some(prev_region_end..self.max_range_end)
    }
}

// === Batch ===

pub struct Batch<'a, A: Arch> {
    pub aspace: &'a mut kmem_core::AddressSpace<A>,
    pub frame_alloc: &'static FrameAllocator,
    range: Range<VirtualAddress>,
    attributes: MemoryAttributes,
    actions: Vec<BBatchAction>,
}

#[derive(Debug)]
enum BBatchAction {
    Map(PhysicalAddress, usize),
}

impl<A: Arch> Drop for Batch<'_, A> {
    fn drop(&mut self) {
        if !self.actions.is_empty() {
            tracing::error!("batch was not flushed before dropping");
            // panic_unwind::panic_in_drop!("batch was not flushed before dropping");
        }
    }
}

impl<'a, A: kmem_core::Arch> Batch<'a, A> {
    pub fn new(
        aspace: &'a mut kmem_core::AddressSpace<A>,
        frame_alloc: &'static FrameAllocator,
    ) -> Self {
        Self {
            aspace,
            frame_alloc,
            range: Range::default(),
            attributes: MemoryAttributes::new(),
            actions: vec![],
        }
    }

    pub fn queue_map(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        len: NonZeroUsize,
        attributes: MemoryAttributes,
    ) -> crate::Result<()> {
        debug_assert!(
            len.get().is_multiple_of(arch::PAGE_SIZE),
            "physical address range must be multiple of page size"
        );

        tracing::trace!("appending {phys:?} at {virt:?} with flags {attributes:?}");
        if self.range.end != virt || self.attributes != attributes {
            self.flush()?;
            self.attributes = attributes;
            self.range = Range::from_start_len(virt, len.get());
        } else {
            self.range.end = self.range.end.add(len.get());
        }

        self.actions.push(BBatchAction::Map(phys, len.get()));

        Ok(())
    }

    pub fn flush(&mut self) -> crate::Result<()> {
        if self.actions.is_empty() {
            return Ok(());
        }
        tracing::trace!("flushing batch {:?} {:?}...", self.range, self.actions);

        let mut flush = Flush::new();
        let mut virt = self.range;
        for action in self.actions.drain(..) {
            match action {
                // Safety: we have checked all the invariants
                BBatchAction::Map(phys, len) => unsafe {
                    self.aspace.map_contiguous(
                        virt.clone(),
                        phys,
                        self.attributes,
                        self.frame_alloc,
                        &mut flush,
                    )?;
                    virt.start = virt.start.add(len);
                },
            }
        }
        flush.flush(self.aspace.arch());

        self.range = Range::from_start_len(self.range.end, 0);
        Ok(())
    }

    pub fn ignore(&mut self) {
        self.actions.clear();
    }
}

#[cfg(test)]
mod tests {
    use wavltree::WAVLTree;

    use super::*;

    #[test]
    fn gaps() {
        let mut tree: WAVLTree<AddressSpaceRegion> = WAVLTree::new();
        let mut addr = VirtualAddress::new(0x000000000000b000);

        for _ in 0..10 {
            tree.insert(Box::pin(AddressSpaceRegion::new_wired(
                Range::from_start_len(addr, 4096),
                MemoryAttributes::new(),
                None,
            )));
            addr = addr.add(11 * 4096);
        }

        let expected_regions = [
            VirtualAddress::new(0x000000000000b000)..VirtualAddress::new(0x000000000000c000),
            VirtualAddress::new(0x0000000000016000)..VirtualAddress::new(0x0000000000017000),
            VirtualAddress::new(0x0000000000021000)..VirtualAddress::new(0x0000000000022000),
            VirtualAddress::new(0x000000000002c000)..VirtualAddress::new(0x000000000002d000),
            VirtualAddress::new(0x0000000000037000)..VirtualAddress::new(0x0000000000038000),
            VirtualAddress::new(0x0000000000042000)..VirtualAddress::new(0x0000000000043000),
            VirtualAddress::new(0x000000000004d000)..VirtualAddress::new(0x000000000004e000),
            VirtualAddress::new(0x0000000000058000)..VirtualAddress::new(0x0000000000059000),
            VirtualAddress::new(0x0000000000063000)..VirtualAddress::new(0x0000000000064000),
            VirtualAddress::new(0x000000000006e000)..VirtualAddress::new(0x000000000006f000),
        ];

        let regions: Vec<_> = tree.iter().map(|region| region.range().clone()).collect();
        assert_eq!(&expected_regions, regions.as_slice());

        let expected_gaps = [
            VirtualAddress::new(usize::MIN)..VirtualAddress::new(0x000000000000b000),
            VirtualAddress::new(0x000000000000c000)..VirtualAddress::new(0x0000000000016000),
            VirtualAddress::new(0x0000000000017000)..VirtualAddress::new(0x0000000000021000),
            VirtualAddress::new(0x0000000000022000)..VirtualAddress::new(0x000000000002c000),
            VirtualAddress::new(0x000000000002d000)..VirtualAddress::new(0x0000000000037000),
            VirtualAddress::new(0x0000000000038000)..VirtualAddress::new(0x0000000000042000),
            VirtualAddress::new(0x0000000000043000)..VirtualAddress::new(0x000000000004d000),
            VirtualAddress::new(0x000000000004e000)..VirtualAddress::new(0x0000000000058000),
            VirtualAddress::new(0x0000000000059000)..VirtualAddress::new(0x0000000000063000),
            VirtualAddress::new(0x0000000000064000)..VirtualAddress::new(0x000000000006e000),
            VirtualAddress::new(0x000000000006f000)..VirtualAddress::new(usize::MAX),
        ];

        let mut gaps = Gaps {
            layout: Layout::from_size_align(4096, 4096).unwrap(),
            stack: vec![],
            prev_region_end: Some(VirtualAddress::MIN),
            max_range_end: VirtualAddress::MAX,
        };
        gaps.push_left_nodes(tree.root().get().unwrap());

        let gaps: Vec<_> = gaps.map(|region| region).collect();
        assert_eq!(&expected_gaps, gaps.as_slice());
    }
}
