// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

extern crate std;

use core::alloc::{Allocator, Layout};
use core::marker::PhantomData;
use core::mem;
use core::num::NonZeroUsize;
use core::ptr::NonNull;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use crate::arch::{Arch, PageTableLevel};
use crate::{AllocError, FrameAllocator, PhysicalAddress, VirtualAddress};

pub struct TestFrameAllocator<A> {
    allocations: Mutex<HashMap<PhysicalAddress, (NonNull<[u8]>, Layout)>>,
    min_block_size: NonZeroUsize,
    max_block_size: Option<NonZeroUsize>,
    _phantom: PhantomData<A>,
}

impl<A: Arch> TestFrameAllocator<A> {
    pub fn new() -> Self {
        Self {
            allocations: Mutex::new(HashMap::new()),
            min_block_size: NonZeroUsize::new(A::PAGE_SIZE).unwrap(),
            max_block_size: None,
            _phantom: PhantomData,
        }
    }

    pub fn with_min_block_size(mut self, min_block_size: NonZeroUsize) -> Self {
        self.min_block_size = min_block_size;
        self
    }

    pub fn with_max_block_size(mut self, max_block_size: NonZeroUsize) -> Self {
        self.max_block_size = Some(max_block_size);
        self
    }

    pub fn allocations(&self) -> MutexGuard<'_, HashMap<PhysicalAddress, (NonNull<[u8]>, Layout)>> {
        self.allocations.lock().unwrap()
    }
}

unsafe impl<A: Arch> FrameAllocator<A> for TestFrameAllocator<A> {
    fn size_hint(&self) -> (NonZeroUsize, Option<NonZeroUsize>) {
        (self.min_block_size, self.max_block_size)
    }

    fn allocate_contiguous(&self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        let mut allocations = self.allocations.lock().unwrap();

        let ptr = std::alloc::Global
            .allocate(layout)
            .map_err(|_| AllocError)?;

        let addr = PhysicalAddress::from_non_null(ptr);
        allocations.insert(addr, (ptr, layout));

        Ok(addr)
    }

    unsafe fn deallocate(&self, block: PhysicalAddress, layout: Layout) {
        let mut allocations = self.allocations.lock().unwrap();

        let (ptr, expected_layout) = allocations.remove(&block).unwrap();

        debug_assert!(layout == expected_layout);

        unsafe { std::alloc::Global.deallocate(ptr.cast(), layout) };
    }
}

impl<A> Drop for TestFrameAllocator<A> {
    fn drop(&mut self) {
        let allocations = mem::take(self.allocations.get_mut().unwrap());

        for (_, (ptr, layout)) in allocations {
            unsafe { std::alloc::Global.deallocate(ptr.cast(), layout) };
        }
    }
}

pub struct TestArch<A: Arch> {
    arch: A,
    root_table: Option<PhysicalAddress>,
}

impl<A: Arch> TestArch<A> {
    pub fn new(arch: A) -> Self {
        Self {
            arch,
            root_table: None,
        }
    }
}

impl<A: Arch> Arch for TestArch<A> {
    const PAGE_SIZE: usize = A::PAGE_SIZE;
    const PAGE_TABLE_LEVELS: &'static [PageTableLevel] = A::PAGE_TABLE_LEVELS;
    type PageTableEntry = A::PageTableEntry;

    fn phys_to_virt(phys: PhysicalAddress) -> VirtualAddress {
        VirtualAddress::from_phys(phys, 0) // identity mapping
    }

    unsafe fn active_table(&mut self) -> PhysicalAddress {
        self.root_table.expect("not active page table")
    }

    unsafe fn set_active_table(&mut self, addr: PhysicalAddress) {
        self.root_table = Some(addr);
    }
}
