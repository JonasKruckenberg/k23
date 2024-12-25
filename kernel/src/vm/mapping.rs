use crate::vm::{PageFaultFlags, PagedVmo, Vmo, WiredVmo, FRAME_ALLOC};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::any::Any;
use core::fmt::Formatter;
use core::mem::offset_of;
use core::num::NonZeroUsize;
use core::ops::{DerefMut, Range};
use core::pin::Pin;
use core::ptr::NonNull;
use core::{cmp, fmt};
use mmu::{arch, AddressRangeExt, VirtualAddress};
use pin_project_lite::pin_project;
use sync::Mutex;
use wavltree::Side;
use crate::vm::aspace::Batch;

pin_project! {
    pub struct Mapping {
        pub links: wavltree::Links<Mapping>,
        pub min_first_byte: VirtualAddress,
        pub max_last_byte: VirtualAddress,
        pub max_gap: usize,
        pub aspace: Arc<Mutex<mmu::AddressSpace>>,
        pub range: Range<VirtualAddress>,
        pub flags: mmu::Flags,
        pub name: String,
        // TODO replace with ksharded_slab::pool:OwnedRef
        pub vmo: Arc<dyn Vmo>,
        pub vmo_offset: usize
    }

    impl PinnedDrop for Mapping {
        fn drop(this: Pin<&mut Self>) {
            let mut frame_alloc = FRAME_ALLOC.get().unwrap().lock();

            let this = this.project();
            let mut aspace = this.aspace.lock();
            let mut flush = mmu::Flush::empty(aspace.asid());


            if let Err(err) = aspace.unmap(
                frame_alloc.deref_mut(),
                this.range.start,
                NonZeroUsize::new(
                this.range.size()).unwrap(),
                &mut flush
            ) {
                panic_unwind::panic_in_drop!("failed to unmap {}: {err}", this.name);
            }
        }
    }
}

impl fmt::Debug for Mapping {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mapping")
            .field("range", &self.range)
            .field("flags", &self.flags)
            .field("min_first_byte", &self.min_first_byte)
            .field("max_last_byte", &self.max_last_byte)
            .field("max_gap", &self.max_gap)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Mapping {
    pub fn new(
        mmu_aspace: Arc<Mutex<mmu::AddressSpace>>,
        range: Range<VirtualAddress>,
        flags: mmu::Flags,
        vmo: Arc<dyn Vmo>,
        vmo_offset: usize,
        name: String,
    ) -> Pin<Box<Self>> {
        Box::pin(Self {
            links: wavltree::Links::default(),
            min_first_byte: range.start,
            max_last_byte: range.end,
            max_gap: 0,
            aspace: mmu_aspace,
            range,
            flags,
            vmo,
            vmo_offset,
            name,
        })
    }

    pub fn map_range(&self, batch: &mut Batch, range: Range<VirtualAddress>) -> crate::Result<()> {
        assert!(!range.is_empty());
        assert!(self.range.start <= range.start && self.range.end >= range.end);
        
        let phys = self.vmo.lookup_contiguous(self.vmo_offset..self.vmo_offset + range.size())?;
        
        batch.append(range.start, (phys.start, phys.size()), self.flags)?;
        
        Ok(())
    }

    pub fn page_fault(
        mut self: Pin<&mut Self>,
        virt: VirtualAddress,
        flags: PageFaultFlags,
    ) -> crate::Result<()> {
        log::trace!("page fault at {virt:?} flags {flags:?} against {self:?}");
        debug_assert!(virt.is_aligned(arch::PAGE_SIZE));
        debug_assert!(self.range.contains(&virt));

        let mut mmu_flags = mmu::Flags::empty();
        if flags.contains(PageFaultFlags::WRITE) {
            mmu_flags |= mmu::Flags::WRITE;
        } else {
            mmu_flags |= mmu::Flags::READ;
        }
        if flags.contains(PageFaultFlags::INSTRUCTION) {
            mmu_flags |= mmu::Flags::EXECUTE;
        }

        if !self.flags.contains(mmu_flags) {
            let diff = mmu_flags.difference(self.flags);

            if diff.contains(mmu::Flags::WRITE) {
                log::trace!("permission failure: write fault on non-writable region");
            }
            if diff.contains(mmu::Flags::READ) {
                log::trace!("permission failure: read fault on non-readable region");
            }
            if diff.contains(mmu::Flags::EXECUTE) {
                log::trace!("permission failure: execute fault on non-executable region");
            }

            return Err(crate::Error::AccessDenied);
        }

        if let Some(paged) = self.vmo.as_any().downcast_ref::<PagedVmo>() {
            todo!("paged VMO fault")
            //     // TODO
            //     //      IF mapping is backed by paged memory
            //     //          ->
        } else if let Some(wired) = self.vmo.as_any().downcast_ref::<WiredVmo>() {
            todo!("wired VMO fault")
            //     // TODO
            //     //      IF mapping is backed by physical memory
            //     //          -> MAYBE? ensure mapping is materialized into page table
            //     //          -> fail
        } else {
            unreachable!("unknown VMO type");
        }

        todo!()
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
            let left_gap = gap(left.max_last_byte, node.range.start);
            left_max_gap = cmp::max(left_gap, left.max_gap);
            node.min_first_byte = left.min_first_byte;
        } else {
            node.min_first_byte = node.range.start;
        }

        if let Some(right) = right {
            let right = right.as_ref();
            let right_gap = gap(node.range.end, right.min_first_byte);
            right_max_gap = cmp::max(right_gap, unsafe { right.max_gap });
            node.max_last_byte = right.max_last_byte;
        } else {
            node.max_last_byte = node.range.end;
        }

        node.max_gap = cmp::max(left_max_gap, right_max_gap);

        fn gap(left_last_byte: VirtualAddress, right_first_byte: VirtualAddress) -> usize {
            debug_assert!(
                left_last_byte <= right_first_byte,
                "subtraction would underflow: {left_last_byte} >= {right_first_byte}"
            );
            right_first_byte.sub_addr(left_last_byte)
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

unsafe impl wavltree::Linked for Mapping {
    /// Any heap-allocated type that owns an element may be used.
    ///
    /// An element *must not* move while part of an intrusive data
    /// structure. In many cases, `Pin` may be used to enforce this.
    type Handle = Pin<Box<Self>>;

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
        debug_assert_eq!(self.min_first_byte, self.range.start);
        debug_assert_eq!(self.max_last_byte, self.range.end);
        debug_assert_eq!(self.max_gap, 0);
        Self::propagate_to_root(self.links.parent());
    }

    fn after_remove(self: Pin<&mut Self>, parent: Option<NonNull<Self>>) {
        Self::propagate_to_root(parent);
    }

    fn after_rotate(
        mut self: Pin<&mut Self>,
        parent: NonNull<Self>,
        sibling: Option<NonNull<Self>>,
        lr_child: Option<NonNull<Self>>,
        side: Side,
    ) {
        let mut this = self.project();
        let _parent = unsafe { parent.as_ref() };

        *this.min_first_byte = _parent.min_first_byte;
        *this.max_last_byte = _parent.max_last_byte;
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
