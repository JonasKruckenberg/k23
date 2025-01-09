use crate::error::Error;
use crate::vm::address_space::Batch;
use crate::vm::Vmo;
use crate::vm::{PageFaultFlags, Permissions};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::cmp;
use core::mem::offset_of;
use core::pin::Pin;
use core::ptr::NonNull;
use core::range::Range;
use mmu::arch::PAGE_SIZE;
use mmu::{AddressRangeExt, VirtualAddress};
use pin_project::pin_project;
use wavltree::Side;

/// A contiguous region of an address space
#[pin_project]
#[derive(Debug)]
pub struct AddressSpaceRegion {
    /// Links to other regions in the WAVL tree
    pub(super) links: wavltree::Links<AddressSpaceRegion>,
    /// The address range covered by this region and its WAVL tree subtree, used when allocating new regions
    pub(super) max_range: Range<VirtualAddress>,
    /// The largest gap in this subtree, used when allocating new regions
    pub(super) max_gap: usize,
    /// The address range covered by this region
    pub range: Range<VirtualAddress>,
    /// The permissions of this region
    pub permissions: Permissions,
    /// The name of this region, for debugging
    pub name: String,
    /// The Virtual Memory Object backing this region
    pub vmo: Arc<Vmo>,
    pub vmo_offset: usize,
}

impl AddressSpaceRegion {
    pub(crate) fn new(
        range: Range<VirtualAddress>,
        permissions: Permissions,
        vmo: Arc<Vmo>,
        vmo_offset: usize,
        name: String,
    ) -> Pin<Box<Self>> {
        Box::pin(Self {
            links: Default::default(),
            max_range: range,
            max_gap: 0,
            range,
            permissions,
            name,
            vmo,
            vmo_offset,
        })
    }

    pub fn page_fault(
        self: Pin<&mut Self>,
        mmu_aspace: &mut mmu::AddressSpace,
        addr: VirtualAddress,
        flags: PageFaultFlags,
    ) -> crate::Result<()> {
        log::trace!("page fault at {addr:?} flags {flags:?} against {self:?}");
        debug_assert!(addr.is_aligned_to(PAGE_SIZE));
        debug_assert!(self.range.contains(&addr));

        let access_permission = Permissions::from(flags);
        if self.permissions.contains(access_permission) {
            let diff = access_permission.difference(self.permissions);
            // diff being empty here means there is no permission mismatch e.g. a read fault against
            // a read-accessible mapping. Hardware *should* never generate such faults, and for soft
            // faults it is a programmer error. either way, a bug is afoot.
            debug_assert!(
                !diff.is_empty(),
                "triggered page fault against accessible page"
            );

            if diff.contains(Permissions::WRITE) {
                log::trace!("permission failure: write fault on non-writable region");
            }
            if diff.contains(Permissions::READ) {
                log::trace!("permission failure: read fault on non-readable region");
            }
            if diff.contains(Permissions::EXECUTE) {
                log::trace!("permission failure: execute fault on non-executable region");
            }

            return Err(Error::AccessDenied);
        }

        let vmo_relative_offset = addr.checked_sub_addr(self.range.start).unwrap();

        let mut batch = Batch::new(mmu_aspace);

        match self.vmo.as_ref() {
            Vmo::Wired(vmo) => {
                let range_phys = vmo
                    .lookup_contiguous(Range::from(
                        vmo_relative_offset..vmo_relative_offset.checked_add(PAGE_SIZE).unwrap(),
                    ))
                    .expect("contiguous lookup for wired VMOs should never fail");

                log::trace!("CASE wired VMO => materializing permission changes for range {range_phys:?} {:?}", self.permissions);
                batch.append(
                    self.range.start,
                    (range_phys.start, range_phys.size()),
                    self.permissions.into(),
                )?;
            }
            Vmo::Paged(vmo) => {
                let frame = vmo.require_frame(vmo_relative_offset, flags.cause_is_write())?;

                batch.append(
                    self.range.start,
                    (frame.addr(), PAGE_SIZE),
                    self.permissions.into(),
                )?;
            }
        }

        batch.flush()?;

        Ok(())
    }

    unsafe fn update(
        mut node: NonNull<Self>,
        left: Option<NonNull<Self>>,
        right: Option<NonNull<Self>>,
    ) {
        let node = node.as_mut();
        let mut left_max_gap = 0;
        let mut right_max_gap = 0;

        if let Some(left) = left {
            let left = left.as_ref();
            let left_gap = gap(left.max_range.end, node.range.start);
            left_max_gap = cmp::max(left_gap, left.max_gap);
            node.max_range.start = left.max_range.start;
        } else {
            node.max_range.start = node.range.start;
        }

        if let Some(right) = right {
            let right = right.as_ref();
            let right_gap = gap(node.range.end, right.max_range.start);
            right_max_gap = cmp::max(right_gap, right.max_gap);
            node.max_range.end = right.max_range.end;
        } else {
            node.max_range.end = node.range.end;
        }

        node.max_gap = cmp::max(left_max_gap, right_max_gap);

        fn gap(left_last_byte: VirtualAddress, right_first_byte: VirtualAddress) -> usize {
            right_first_byte
                .checked_sub_addr(left_last_byte)
                .unwrap_or_default() // TODO use saturating_sub_addr
        }
    }

    fn propagate_to_root(mut maybe_node: Option<NonNull<Self>>) {
        while let Some(node) = maybe_node {
            let links = unsafe { &node.as_ref().links };
            unsafe {
                Self::update(node, links.left(), links.right());
            }
            maybe_node = links.parent();
        }
    }
}

unsafe impl wavltree::Linked for AddressSpaceRegion {
    /// Any heap-allocated type that owns an element may be used.
    ///
    /// An element *must not* move while part of an intrusive data
    /// structure. In many cases, `Pin` may be used to enforce this.
    type Handle = Pin<Box<Self>>; // TODO better handle type

    type Key = VirtualAddress;

    /// Convert an owned `Handle` into a raw pointer
    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        unsafe { NonNull::from(Box::leak(Pin::into_inner_unchecked(handle))) }
    }

    /// Convert a raw pointer back into an owned `Handle`.
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // Safety: `NonNull` *must* be constructed from a pinned reference
        // which the tree implementation upholds.
        Pin::new_unchecked(Box::from_raw(ptr.as_ptr()))
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
        debug_assert_eq!(self.max_range.start, self.range.start);
        debug_assert_eq!(self.max_range.end, self.range.end);
        debug_assert_eq!(self.max_gap, 0);
        Self::propagate_to_root(self.links.parent());
    }

    fn after_remove(self: Pin<&mut Self>, parent: Option<NonNull<Self>>) {
        Self::propagate_to_root(parent);
    }

    fn after_rotate(
        self: Pin<&mut Self>,
        parent: NonNull<Self>,
        sibling: Option<NonNull<Self>>,
        lr_child: Option<NonNull<Self>>,
        side: Side,
    ) {
        let this = self.project();
        let _parent = unsafe { parent.as_ref() };

        this.max_range.start = _parent.max_range.start;
        this.max_range.end = _parent.max_range.end;
        *this.max_gap = _parent.max_gap;

        if side == Side::Left {
            unsafe {
                Self::update(parent, sibling, lr_child);
            }
        } else {
            unsafe {
                Self::update(parent, lr_child, sibling);
            }
        }
    }
}
