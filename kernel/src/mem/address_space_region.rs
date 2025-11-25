// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::alloc::Layout;
use core::mem::offset_of;
use core::num::NonZeroUsize;
use core::ops::Range;
use core::pin::Pin;
use core::ptr::NonNull;
use core::{cmp, mem};

use anyhow::bail;
use kmem_core::{AddressRangeExt, MemoryAttributes, PhysicalAddress, VirtualAddress};
use pin_project::pin_project;
use spin::LazyLock;

use crate::arch;
use crate::mem::frame_alloc::FrameAllocator;
use crate::mem::{Batch, PageFaultFlags, Vmo};

/// A contiguous region of an address space
#[pin_project]
#[derive(Debug)]
pub struct AddressSpaceRegion {
    /// The address range covered by this region
    pub range: Range<VirtualAddress>,
    /// The memory attributes of this region
    pub attributes: MemoryAttributes,
    /// The name of this region, for debugging
    pub name: Option<String>,
    /// The Virtual Memory Object backing this region
    pub vmo: Arc<Vmo>,
    pub vmo_offset: usize,

    /// The address range covered by this region and its WAVL tree subtree, used when allocating new regions
    subtree_range: Range<VirtualAddress>,
    /// The largest gap in this subtree, used when allocating new regions
    max_gap: usize,

    /// Links to other regions in the WAVL tree
    links: wavltree::Links<AddressSpaceRegion>,
}

impl AddressSpaceRegion {
    pub fn new_zeroed(
        frame_alloc: &'static FrameAllocator,
        range: Range<VirtualAddress>,
        attributes: MemoryAttributes,
        name: Option<String>,
    ) -> Self {
        Self {
            range: range.clone(),
            attributes,
            name,
            vmo: Arc::new(Vmo::new_zeroed(frame_alloc)),
            vmo_offset: 0,
            subtree_range: range,
            max_gap: 0,
            links: wavltree::Links::default(),
        }
    }

    pub fn new_phys(
        virt: Range<VirtualAddress>,
        attributes: MemoryAttributes,
        phys: Range<PhysicalAddress>,
        name: Option<String>,
    ) -> AddressSpaceRegion {
        Self {
            range: virt.clone(),
            attributes,
            name,
            vmo: Arc::new(Vmo::new_phys(phys)),
            vmo_offset: 0,
            subtree_range: virt,
            max_gap: 0,
            links: wavltree::Links::default(),
        }
    }

    pub fn new_wired(
        range: Range<VirtualAddress>,
        attributes: MemoryAttributes,
        name: Option<String>,
    ) -> AddressSpaceRegion {
        static WIRED_VMO: LazyLock<Arc<Vmo>> = LazyLock::new(|| Arc::new(Vmo::Wired));

        Self {
            range: range.clone(),
            attributes,
            name,
            vmo: WIRED_VMO.clone(),
            vmo_offset: 0,
            subtree_range: range,
            max_gap: 0,
            links: wavltree::Links::default(),
        }
    }

    // #[expect(tail_expr_drop_order, reason = "")]
    // pub(crate) fn new(
    //     range: Range<VirtualAddress>,
    //     permissions: Permissions,
    //     vmo: Arc<Vmo>,
    //     vmo_offset: usize,
    //     name: Option<String>,
    // ) -> Pin<Box<Self>> {
    //     Box::pin(Self {
    //         links: wavltree::Links::default(),
    //         max_range: range,
    //         max_gap: 0,
    //         range,
    //         permissions,
    //         name,
    //         vmo,
    //         vmo_offset,
    //     })
    // }

    pub fn commit<A: kmem_core::Arch>(
        &self,
        batch: &mut Batch<A>,
        range: Range<VirtualAddress>,
        will_write: bool,
    ) -> crate::Result<()> {
        let vmo_relative_range = Range {
            start: range.start.offset_from_unsigned(self.range.start),
            end: range.end.offset_from_unsigned(self.range.start),
        };

        match self.vmo.as_ref() {
            Vmo::Wired => unreachable!(),
            Vmo::Phys(vmo) => {
                let range_phys = vmo
                    .lookup_contiguous(vmo_relative_range)
                    .expect("contiguous lookup for wired VMOs should never fail");

                batch.queue_map(
                    range.start,
                    range_phys.start,
                    NonZeroUsize::new(range_phys.len()).unwrap(),
                    self.attributes,
                )?;
            }
            Vmo::Paged(vmo) => {
                if will_write {
                    let mut vmo = vmo.write();

                    for addr in range.step_by(arch::PAGE_SIZE) {
                        debug_assert!(addr.is_aligned_to(arch::PAGE_SIZE));
                        let vmo_relative_offset = addr.offset_from_unsigned(self.range.start);
                        let frame =
                            vmo.require_owned_frame(vmo_relative_offset, batch.aspace.arch())?;
                        batch.queue_map(
                            addr,
                            frame.addr(),
                            NonZeroUsize::new(arch::PAGE_SIZE).unwrap(),
                            self.attributes,
                        )?;
                    }
                } else {
                    let mut vmo = vmo.write();

                    for addr in range.into_iter().step_by(arch::PAGE_SIZE) {
                        debug_assert!(addr.is_aligned_to(arch::PAGE_SIZE));
                        let vmo_relative_offset = addr.offset_from_unsigned(self.range.start);
                        let frame = vmo.require_read_frame(vmo_relative_offset)?;
                        batch.queue_map(
                            addr,
                            frame.addr(),
                            NonZeroUsize::new(arch::PAGE_SIZE).unwrap(),
                            self.attributes.difference(Permissions::WRITE).into(),
                        )?;
                    }
                }
            }
        }

        Ok(())
    }

    // TODO this method should be changed to accept an `arch::AddressSpace` and flusher and perform
    //  the unmapping by itself instead of the `AddressSpace` doing it
    #[expect(clippy::unnecessary_wraps, reason = "TODO")]
    pub fn unmap(&self, range: Range<VirtualAddress>) -> crate::Result<()> {
        match self.vmo.as_ref() {
            Vmo::Wired => panic!("cannot unmap wired frames"),
            Vmo::Phys(_) => {
                // physical frames aren't managed by anyone, so there is nothing to free here
                // the unmap handling in `AddressSpace` will take care of the unmapping
            }
            Vmo::Paged(vmo) => {
                let vmo_relative_range = Range {
                    start: range.start.offset_from_unsigned(self.range.start) + self.vmo_offset,
                    end: range.end.offset_from_unsigned(self.range.start) + self.vmo_offset,
                };

                let mut vmo = vmo.write();
                vmo.free_frames(vmo_relative_range);
            }
        }

        Ok(())
    }

    pub fn page_fault<A: kmem_core::Arch>(
        self: Pin<&mut Self>,
        batch: &mut Batch<A>,
        addr: VirtualAddress,
        flags: PageFaultFlags,
    ) -> crate::Result<()> {
        tracing::trace!(addr=%addr,flags=%flags,name=?self.name, "page fault");
        debug_assert!(addr.is_aligned_to(arch::PAGE_SIZE));
        debug_assert!(self.range.contains(&addr));

        // Check that the access (read,write or execute) is permitted given this region's permissions
        let access_permission = Permissions::from(flags);
        let diff = access_permission.difference(self.attributes);
        if !diff.is_empty() {
            // diff being empty here means there is no permission mismatch e.g. a read fault against
            // a read-accessible mapping. Hardware *should* never generate such faults, and for soft
            // faults it is a programmer error. either way, a bug is afoot.
            debug_assert!(
                !diff.is_empty(),
                "triggered page fault against accessible page"
            );

            if diff.contains(Permissions::WRITE) {
                tracing::trace!("permission failure: write fault on non-writable region");
            }
            if diff.contains(Permissions::READ) {
                tracing::trace!("permission failure: read fault on non-readable region");
            }
            if diff.contains(Permissions::EXECUTE) {
                tracing::trace!("permission failure: execute fault on non-executable region");
            }

            bail!("requested permissions must be R^X");
        }

        // At this point we know that the access was legal, so either we faulted because the Frame
        // was missing because we paged it out (THIS IS NOT POSSIBLE YET) or the actual MMU flags
        // didn't match the logical permissions.
        // This either means MMU flags were set incorrectly (DOES THIS EVEN HAPPEN?) or - and this
        // is the most common case - we attempted to write to a non-writable region which means we
        // need to do copy-on-write.
        //
        // There is another small optimization here: The physical memory can also be *Wired* which means
        // it is always mapped, cannot be paged-out, and also doesn't support COW. This is used to
        // simplify handling of regions like kernel memory which must always be present anyway.

        let vmo_relative_offset = addr.offset_from_unsigned(self.range.start);

        match self.vmo.as_ref() {
            Vmo::Wired => unreachable!("Wired VMO can never page fault"),
            Vmo::Phys(vmo) => {
                let range_phys = vmo
                    .lookup_contiguous(vmo_relative_offset..vmo_relative_offset + arch::PAGE_SIZE)
                    .expect("contiguous lookup for wired VMOs should never fail");

                batch.queue_map(
                    addr,
                    range_phys.start,
                    NonZeroUsize::new(range_phys.len()).unwrap(),
                    self.attributes,
                )?;
            }
            Vmo::Paged(vmo) => {
                if flags.cause_is_write() {
                    let mut vmo = vmo.write();

                    let frame =
                        vmo.require_owned_frame(vmo_relative_offset, batch.aspace.arch())?;
                    batch.queue_map(
                        addr,
                        frame.addr(),
                        NonZeroUsize::new(arch::PAGE_SIZE).unwrap(),
                        self.attributes,
                    )?;
                } else {
                    let mut vmo = vmo.write();

                    let frame = vmo.require_read_frame(vmo_relative_offset)?;
                    batch.queue_map(
                        addr,
                        frame.addr(),
                        NonZeroUsize::new(arch::PAGE_SIZE).unwrap(),
                        self.attributes.difference(Permissions::WRITE).into(),
                    )?;
                }

                // TODO fault-ahead or fault-behind here
                //  see #282 and #283 for details
            }
        }

        Ok(())
    }

    /// Returns this regions address range.
    pub const fn range(&self) -> &Range<VirtualAddress> {
        &self.range
    }

    /// Returns the largest range covered by this region and all it's binary-search-tree children,
    /// used during gap-searching.
    pub const fn subtree_range(&self) -> &Range<VirtualAddress> {
        &self.subtree_range
    }

    /// Returns `true` if this nodes subtree contains a gap suitable for the given `layout`, used
    /// during gap-searching.
    pub fn suitable_gap_in_subtree(&self, layout: Layout) -> bool {
        // we need the layout to be padded to alignment
        debug_assert!(layout.size().is_multiple_of(layout.align()));

        self.max_gap >= layout.size()
    }

    /// Returns the left child node in the search tree of regions, used during gap-searching.
    pub fn left_child(&self) -> Option<&Self> {
        // Safety: we have to trust the intrusive tree implementation here
        Some(unsafe { self.links.left()?.as_ref() })
    }

    /// Returns the right child node in the search tree of regions, used during gap-searching.
    pub fn right_child(&self) -> Option<&Self> {
        // Safety: we have to trust the intrusive tree implementation here
        Some(unsafe { self.links.right()?.as_ref() })
    }

    /// Returns the parent node in the search tree of regions, used during gap-searching.
    pub fn parent(&self) -> Option<&Self> {
        // Safety: we have to trust the intrusive tree implementation here
        Some(unsafe { self.links.parent()?.as_ref() })
    }

    /// Update the gap search metadata of this region. This method is called in the [`wavltree::Linked`]
    /// implementation below after each tree mutation that impacted this node or its subtree in some way
    /// (insertion, rotation, deletion).
    ///
    /// Returns `true` if this nodes metadata changed.
    fn update_gap_metadata(
        mut node: NonNull<Self>,
        left: Option<NonNull<Self>>,
        right: Option<NonNull<Self>>,
    ) -> bool {
        fn gap(left_last_byte: VirtualAddress, right_first_byte: VirtualAddress) -> usize {
            right_first_byte.offset_from_unsigned(left_last_byte)
        }

        // Safety: we have to trust the intrusive tree implementation
        let node = unsafe { node.as_mut() };
        let mut left_max_gap = 0;
        let mut right_max_gap = 0;

        // recalculate the subtree_range start
        let old_subtree_range_start = if let Some(left) = left {
            // Safety: we have to trust the intrusive tree implementation
            let left = unsafe { left.as_ref() };
            let left_gap = gap(left.subtree_range.end, node.range.start);
            left_max_gap = cmp::max(left_gap, left.max_gap);
            mem::replace(&mut node.subtree_range.start, left.subtree_range.start)
        } else {
            mem::replace(&mut node.subtree_range.start, node.range.start)
        };

        // recalculate the subtree range end
        let old_subtree_range_end = if let Some(right) = right {
            // Safety: we have to trust the intrusive tree implementation
            let right = unsafe { right.as_ref() };
            let right_gap = gap(node.range.end, right.subtree_range.start);
            right_max_gap = cmp::max(right_gap, right.max_gap);
            mem::replace(&mut node.subtree_range.end, right.subtree_range.end)
        } else {
            mem::replace(&mut node.subtree_range.end, node.range.end)
        };

        // recalculate the map_gap
        let old_max_gap = mem::replace(&mut node.max_gap, cmp::max(left_max_gap, right_max_gap));

        old_max_gap != node.max_gap
            || old_subtree_range_start != node.subtree_range.start
            || old_subtree_range_end != node.subtree_range.end
    }

    // Propagate metadata updates to this regions parent in the search tree. If we had to update
    // our metadata the parent must update its metadata too.
    fn propagate_update_to_parent(mut maybe_node: Option<NonNull<Self>>) {
        while let Some(node) = maybe_node {
            // Safety: we have to trust the intrusive tree implementation
            let links = unsafe { &node.as_ref().links };
            let changed = Self::update_gap_metadata(node, links.left(), links.right());

            // if the metadata didn't actually change, we don't need to recalculate parents
            if !changed {
                return;
            }

            maybe_node = links.parent();
        }
    }
}

// Safety: unsafe trait
unsafe impl wavltree::Linked for AddressSpaceRegion {
    /// Any heap-allocated type that owns an element may be used.
    ///
    /// An element *must not* move while part of an intrusive data
    /// structure. In many cases, `Pin` may be used to enforce this.
    type Handle = Pin<Box<Self>>; // TODO better handle type

    type Key = VirtualAddress;

    /// Convert an owned `Handle` into a raw pointer
    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        // Safety: wavltree treats the ptr as pinned
        unsafe { NonNull::from(Box::leak(Pin::into_inner_unchecked(handle))) }
    }

    /// Convert a raw pointer back into an owned `Handle`.
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // Safety: `NonNull` *must* be constructed from a pinned reference
        // which the tree implementation upholds.
        unsafe { Pin::new_unchecked(Box::from_raw(ptr.as_ptr())) }
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<wavltree::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }

    fn get_key(&self) -> &Self::Key {
        &self.range.start
    }

    fn after_insert(self: Pin<&mut Self>) {
        debug_assert_eq!(self.subtree_range.start, self.range.start);
        debug_assert_eq!(self.subtree_range.end, self.range.end);
        debug_assert_eq!(self.max_gap, 0);
        Self::propagate_update_to_parent(self.links.parent());
    }

    fn after_remove(self: Pin<&mut Self>, parent: Option<NonNull<Self>>) {
        Self::propagate_update_to_parent(parent);
    }

    fn after_rotate(
        self: Pin<&mut Self>,
        parent: NonNull<Self>,
        sibling: Option<NonNull<Self>>,
        lr_child: Option<NonNull<Self>>,
        side: wavltree::Side,
    ) {
        let this = self.project();
        // Safety: caller ensures ptr is valid
        let _parent = unsafe { parent.as_ref() };

        this.subtree_range.start = _parent.subtree_range.start;
        this.subtree_range.end = _parent.subtree_range.end;
        *this.max_gap = _parent.max_gap;

        if side == wavltree::Side::Left {
            Self::update_gap_metadata(parent, sibling, lr_child);
        } else {
            Self::update_gap_metadata(parent, lr_child, sibling);
        }
    }
}
