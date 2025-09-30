// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

pub mod riscv64;
pub mod bare;

use core::alloc::Layout;
use crate::{MemoryAttributes, PhysicalAddress, VirtualAddress};

pub trait Arch {
    const PAGE_SIZE: usize;
    const PAGE_TABLE_LEVELS: &'static [PageTableLevel];
    const PAGE_LAYOUT: Layout = unsafe { Layout::from_size_align_unchecked(Self::PAGE_SIZE, Self::PAGE_SIZE) };

    type PageTableEntry: PageTableEntry;

    fn phys_to_virt(phys: PhysicalAddress) -> VirtualAddress;

    unsafe fn active_table(&self) -> PhysicalAddress;
    unsafe fn set_active_table(&self, addr: PhysicalAddress);
}

pub trait PageTableEntry: Copy {
    fn new_leaf(phys: PhysicalAddress, attributes: MemoryAttributes) -> Self;
    fn new_table(phys: PhysicalAddress) -> Self;
    fn new_empty() -> Self;

    fn is_vacant(&self) -> bool;
    fn is_leaf(&self) -> bool;

    fn address(&self) -> PhysicalAddress;
    unsafe fn set_address(&mut self, address: PhysicalAddress);

    fn attributes(&self) -> MemoryAttributes;
    unsafe fn set_attributes(&mut self, attributes: MemoryAttributes);
}

#[derive(Debug, Clone, Copy)]
pub struct PageTableLevel {
    /// The name of the page table level, for debugging purposes
    name: &'static str,
    /// The number of entries in this page table level
    entries: usize,
    index_shift: u32,
    supports_leaf: bool,
    block_size: usize,
}

impl PageTableLevel {
    pub(crate) const fn from_parts(
        name: &'static str,
        entries: usize,
        index_shift: u32,
        supports_leaf: bool,
        block_size: usize,
    ) -> Self {
        Self {
            name,
            entries,
            index_shift,
            supports_leaf,
            block_size,
        }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub const fn entries(&self) -> usize {
        self.entries
    }

    pub const fn supports_leaf(&self) -> bool {
        self.supports_leaf
    }

    pub const fn block_size(&self) -> usize {
        self.block_size
    }

    pub(crate) fn table_index(&self, virt: VirtualAddress) -> usize {
        let idx = (virt.get() & (1 << self.entries) - 1) >> self.index_shift;
        assert!(idx < self.entries);
        idx
    }
}

struct PageTableLevelsBuilder<const N: usize> {
    levels: [PageTableLevel; N],
    index_shift: u32,
    page_size: usize
}

impl PageTableLevelsBuilder<0> {
    pub const fn with_page_size(page_size: usize) -> Self {
        Self {
            levels: [],
            index_shift: page_size.ilog2(),
            page_size
        }
    }
}

impl<const N: usize> PageTableLevelsBuilder<N> {
    pub const fn finish(self) -> [PageTableLevel; N] {
        self.levels
    }
}

macro_rules! impl_lvl {
    ($FROM:literal => $TO:literal, [$($lvl:ident),*]) => {
        impl PageTableLevelsBuilder<$FROM> {
            pub const fn with_level(
                self,
                name: &'static str,
                entries: usize,
                supports_leaf: bool,
            ) -> PageTableLevelsBuilder<$TO> {
                let Self { levels: [$($lvl),*], index_shift, page_size } = self;

                PageTableLevelsBuilder {
                    levels: [
                        $($lvl,)*
                        PageTableLevel::from_parts(
                            name,
                            entries,
                            index_shift,
                            supports_leaf,
                            self.page_size.pow($TO)
                        )
                    ],
                    index_shift: index_shift + entries.ilog2(),
                    page_size
                }
            }
        }
    };
}

impl_lvl!(0 => 1, []);
impl_lvl!(1 => 2, [l0]);
impl_lvl!(2 => 3, [l0, l1]);
// impl_lvl!(3 => 4, [l0, l1, l2]);
// impl_lvl!(4 => 5, [l0, l1, l2, l3]);
