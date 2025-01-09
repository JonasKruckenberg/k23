mod bootstrap;

use crate::{PhysicalAddress, VirtualAddress};
use core::alloc::Layout;
use core::ptr;

pub use bootstrap::{BootstrapAllocator, FrameUsage, FreeRegions, UsedRegions};

pub trait FrameAllocator {
    fn allocate_one(&mut self) -> Option<PhysicalAddress>;
    fn allocate_one_zeroed(&mut self) -> Option<PhysicalAddress>;
    fn allocate_contiguous(&mut self, layout: Layout) -> Option<PhysicalAddress>;
    fn deallocate_contiguous(&mut self, addr: PhysicalAddress, layout: Layout);
    fn allocate_contiguous_zeroed(&mut self, layout: Layout) -> Option<PhysicalAddress>;
    fn allocate_partial(&mut self, layout: Layout) -> Option<(PhysicalAddress, usize)>;
}

pub trait FramesIterator: Iterator<Item = (PhysicalAddress, usize)> {
    fn alloc_mut(&mut self) -> &mut dyn FrameAllocator;
}

pub struct NonContiguousFrames<'a> {
    alloc: &'a mut dyn FrameAllocator,
    remaining: usize,
    align: usize,
    zeroed: Option<VirtualAddress>,
}

impl<'a> NonContiguousFrames<'a> {
    pub fn new(alloc: &'a mut dyn FrameAllocator, layout: Layout) -> Self {
        Self {
            alloc,
            remaining: layout.size(),
            align: layout.align(),
            zeroed: None,
        }
    }
    pub fn new_zeroed(
        alloc: &'a mut dyn FrameAllocator,
        layout: Layout,
        phys_offset: VirtualAddress,
    ) -> Self {
        Self {
            alloc,
            remaining: layout.size(),
            align: layout.align(),
            zeroed: Some(phys_offset),
        }
    }
}

impl FramesIterator for NonContiguousFrames<'_> {
    fn alloc_mut(&mut self) -> &mut dyn FrameAllocator {
        self.alloc
    }
}

impl Iterator for NonContiguousFrames<'_> {
    type Item = (PhysicalAddress, usize);

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }

        let layout = Layout::from_size_align(self.remaining, self.align).unwrap();

        if let Some((phys, len)) = self.alloc.allocate_partial(layout) {
            self.remaining -= len;

            if let Some(phys_offset) = self.zeroed {
                let virt = VirtualAddress::from_phys(phys, phys_offset).unwrap();
                unsafe {
                    ptr::write_bytes::<u8>(virt.as_mut_ptr(), 0, layout.size());
                }
            }

            Some((phys, len))
        } else {
            self.remaining = 0;
            None
        }
    }
}
