use core::mem::offset_of;
use core::ptr::NonNull;
use mmu::PhysicalAddress;

#[derive(Debug)]
pub struct Frame {
    links: linked_list::Links<Frame>,
    // The physical address of the frame
    pub phys: PhysicalAddress,
}

impl Frame {
    pub(crate) fn new(phys: PhysicalAddress) -> Self {
        Self {
            links: Default::default(),
            phys,
        }
    }
}

unsafe impl linked_list::Linked for Frame {
    type Handle = NonNull<Frame>;

    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        handle
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        ptr
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<linked_list::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }
}
