use crate::entry::Entry;
use crate::flush::Flush;
use crate::table::Table;
use crate::{AddressRangeExt, Error, FrameAllocator, Mode, PhysicalAddress, VirtualAddress};
use bitflags::Flags;
use core::marker::PhantomData;
use core::ops::Range;

pub struct Mapper<'a, M> {
    asid: usize,
    root_table: VirtualAddress,
    allocator: &'a mut dyn FrameAllocator,
    _m: PhantomData<M>,
}

impl<'a, M: Mode> Mapper<'a, M> {
    pub fn new(asid: usize, allocator: &'a mut dyn FrameAllocator) -> crate::Result<Self> {
        let root_table = allocator.allocate_frame()?;
        let root_table_virt = M::phys_to_virt(root_table);

        Ok(Self {
            asid,
            root_table: root_table_virt,
            allocator,
            _m: PhantomData,
        })
    }

    pub fn from_active(asid: usize, allocator: &'a mut dyn FrameAllocator) -> Self {
        let root_table = M::get_active_table(asid);
        let root_table_virt = M::phys_to_virt(root_table);
        debug_assert!(root_table.0 != 0);

        Self {
            asid,
            root_table: root_table_virt,
            allocator,
            _m: PhantomData,
        }
    }

    pub fn from_address(
        asid: usize,
        root_table: VirtualAddress,
        allocator: &'a mut dyn FrameAllocator,
    ) -> Self {
        Self {
            asid,
            root_table,
            allocator,
            _m: PhantomData,
        }
    }

    pub fn activate(&self) {
        M::activate_table(self.asid, self.root_table);
    }

    pub fn allocator(&self) -> &dyn FrameAllocator {
        self.allocator
    }

    pub fn allocator_mut(&mut self) -> &mut dyn FrameAllocator {
        self.allocator
    }

    pub fn root_table(&self) -> Table<M> {
        unsafe { Table::new(self.root_table, M::PAGE_TABLE_LEVELS - 1) }
    }

    pub fn set_flags_for_range(
        &mut self,
        range_virt: Range<VirtualAddress>,
        flags: M::EntryFlags,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        debug_assert!(
            range_virt.size() >= M::PAGE_SIZE,
            "virtual address range must span be at least one page"
        );

        Self::for_pages_in_range(range_virt.clone(), |i, _, page_size| {
            let virt = range_virt.start.add(i * page_size);

            self.set_flags(virt, flags, flush)
        })
    }

    pub fn set_flags(
        &mut self,
        virt: VirtualAddress,
        flags: M::EntryFlags,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        debug_assert!(
            virt.is_aligned(M::PAGE_SIZE),
            "virtual address is not page aligned"
        );

        let on_leaf = |entry: &mut Entry<M>| {
            assert!(
                !entry.is_vacant(),
                "expected table entry to *not* be vacant, to perform initial mapping use the map_ methods"
            );

            entry.set_address_and_flags(
                entry.get_address(),
                flags.union(M::ENTRY_FLAG_DEFAULT_LEAF),
            );

            flush.extend_range(self.asid, virt..virt.add(M::PAGE_SIZE))?;

            Ok(())
        };

        Self::walk_mut(virt, self.root_table(), on_leaf, |_| Ok(()))
    }

    pub fn map_range_identity(
        &mut self,
        range_phys: Range<PhysicalAddress>,
        flags: M::EntryFlags,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        debug_assert!(
            range_phys.size() >= M::PAGE_SIZE,
            "physical address range must span be at least one page"
        );

        let range_virt = M::phys_to_virt(range_phys.start)..M::phys_to_virt(range_phys.end);

        Self::for_pages_in_range(range_virt.clone(), |i, _, page_size| {
            let virt = range_virt.start.add(i * page_size);
            let phys = range_phys.start.add(i * page_size);

            self.map(virt, phys, flags, flush)
        })
    }

    pub fn map_identity(
        &mut self,
        phys: PhysicalAddress,
        flags: M::EntryFlags,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        let virt = M::phys_to_virt(phys);
        self.map(virt, phys, flags, flush)
    }

    pub fn map_range(
        &mut self,
        range_virt: Range<VirtualAddress>,
        range_phys: Range<PhysicalAddress>,
        flags: M::EntryFlags,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        let len = range_virt.size();
        // make sure both ranges are the same size
        debug_assert_eq!(
            len,
            range_phys.end.0 - range_phys.start.0,
            "cannot map virtual address range to physical address range of different size"
        );

        debug_assert!(
            range_virt.size() >= M::PAGE_SIZE,
            "virtual address range must span be at least one page"
        );
        debug_assert!(
            range_phys.size() >= M::PAGE_SIZE,
            "physical address range must span be at least one page"
        );

        Self::for_pages_in_range(range_virt.clone(), |i, _, page_size| {
            let virt = range_virt.start.add(i * page_size);
            let phys = range_phys.start.add(i * page_size);

            self.map(virt, phys, flags, flush)
        })
    }

    pub fn map(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        flags: M::EntryFlags,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        debug_assert!(
            phys.is_aligned(M::PAGE_SIZE),
            "physical address is not page aligned"
        );
        debug_assert!(
            virt.is_aligned(M::PAGE_SIZE),
            "virtual address is not page aligned"
        );

        let table = self.root_table();

        let on_leaf = |entry: &mut Entry<M>| {
            assert!(
                entry.is_vacant(),
                "expected table entry to be vacant, to remap use  the remap_ methods"
            );

            entry.set_address_and_flags(phys, flags.union(M::ENTRY_FLAG_DEFAULT_LEAF));

            flush.extend_range(self.asid, virt..virt.add(M::PAGE_SIZE))?;

            Ok(())
        };

        let on_node = |entry: &mut Entry<M>| {
            if entry.is_vacant() {
                // allocate a new physical frame to hold the entries children
                let frame_phys = self.allocator.allocate_frame()?;
                entry.set_address_and_flags(frame_phys, M::ENTRY_FLAG_DEFAULT_TABLE);
            }

            Ok(())
        };

        Self::walk_mut(virt, table, on_leaf, on_node)
    }

    pub fn virt_to_phys(&self, virt: VirtualAddress) -> Option<PhysicalAddress> {
        let on_leaf = |entry: &Entry<M>| -> crate::Result<PhysicalAddress> {
            let mut phys = entry.get_address();
            // copy the offset bits from the virtual address
            phys.0 |= virt.0 & M::PAGE_OFFSET_MASK;

            Ok(phys)
        };

        Self::walk(virt, self.root_table(), on_leaf, |_| Ok(())).ok()
    }

    pub fn unmap_forget_range(
        &mut self,
        range_virt: Range<VirtualAddress>,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        Self::for_pages_in_range(range_virt.clone(), |i, _, page_size| {
            let virt = range_virt.start.add(i * page_size);

            self.unmap_forget(virt, flush)?;

            Ok(())
        })
    }

    pub fn unmap_forget(
        &mut self,
        virt: VirtualAddress,
        flush: &mut Flush<M>,
    ) -> crate::Result<PhysicalAddress> {
        debug_assert!(virt.0 % M::PAGE_SIZE == 0);

        let addr = self.unmap_inner(virt, &mut self.root_table(), false)?;
        flush.extend_range(self.asid, virt..virt.add(M::PAGE_SIZE))?;

        Ok(addr)
    }

    pub fn unmap_range(
        &mut self,
        range_virt: Range<VirtualAddress>,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        Self::for_pages_in_range(range_virt.clone(), |i, _, page_size| {
            let virt = range_virt.start.add(i * page_size);

            self.unmap(virt, flush)?;

            Ok(())
        })
    }

    pub fn unmap(
        &mut self,
        virt: VirtualAddress,
        flush: &mut Flush<M>,
    ) -> crate::Result<PhysicalAddress> {
        debug_assert!(virt.0 % M::PAGE_SIZE == 0);

        let addr = self.unmap_inner(virt, &mut self.root_table(), true)?;

        self.allocator.deallocate_frame(addr)?;
        flush.extend_range(self.asid, virt..virt.add(M::PAGE_SIZE))?;

        Ok(addr)
    }

    fn unmap_inner(
        &mut self,
        virt: VirtualAddress,
        table: &mut Table<M>,
        dealloc: bool,
    ) -> crate::Result<PhysicalAddress> {
        let level = table.level();
        let entry = table.entry_mut(table.index_of_virt(virt));

        if entry.is_vacant() {
            return Ok(entry.get_address());
        }

        if level == 0 {
            let address = entry.get_address();
            entry.clear();
            Ok(address)
        } else {
            let table_phys = entry.get_address();

            let table_virt = M::phys_to_virt(table_phys);
            let mut subtable = unsafe { Table::new(table_virt, level - 1) };

            let res = self.unmap_inner(virt, &mut subtable, dealloc)?;

            let is_still_populated = (0..512).map(|j| subtable.entry(j)).any(|e| !e.is_vacant());

            if !is_still_populated {
                if dealloc {
                    let subtable_virt = subtable.addr();
                    let subtable_phys = self.virt_to_phys(subtable_virt).unwrap();
                    self.allocator.deallocate_frame(subtable_phys)?;
                }
                entry.clear();
            }

            Ok(res)
        }
    }

    fn walk_mut<R>(
        virt: VirtualAddress,
        mut table: Table<M>,
        on_leaf: impl FnOnce(&mut Entry<M>) -> crate::Result<R>,
        mut on_node: impl FnMut(&mut Entry<M>) -> crate::Result<()>,
    ) -> crate::Result<R> {
        for lvl in (0..M::PAGE_TABLE_LEVELS).rev() {
            let entry = table.entry_mut(table.index_of_virt(virt));

            if lvl == 0 {
                return on_leaf(entry);
            } else {
                on_node(entry)?;

                if entry.is_vacant() {
                    return Err(Error::NotMapped(virt));
                }

                let table_phys = entry.get_address();
                let table_virt = M::phys_to_virt(table_phys);
                table = unsafe { Table::new(table_virt, table.level() - 1) };
            }
        }

        unreachable!("virtual address was too large to be mapped. This should not be possible");
    }

    fn walk<R>(
        virt: VirtualAddress,
        mut table: Table<M>,
        on_leaf: impl FnOnce(&Entry<M>) -> crate::Result<R>,
        mut on_node: impl FnMut(&Entry<M>) -> crate::Result<()>,
    ) -> crate::Result<R> {
        for lvl in (0..M::PAGE_TABLE_LEVELS).rev() {
            let entry = table.entry(table.index_of_virt(virt));

            if lvl == 0 {
                return on_leaf(entry);
            } else {
                on_node(entry)?;

                if entry.is_vacant() {
                    return Err(Error::NotMapped(virt));
                }

                let table_phys = entry.get_address();
                let table_virt = M::phys_to_virt(table_phys);
                table = unsafe { Table::new(table_virt, table.level() - 1) };
            }
        }

        unreachable!("virtual address was too large to be mapped. This should not be possible");
    }

    fn for_pages_in_range<F>(virt: Range<VirtualAddress>, mut f: F) -> crate::Result<()>
    where
        F: FnMut(usize, usize, usize) -> crate::Result<()>,
    {
        // let find_lvl = |for_range: &Range<VirtualAddress>| -> (usize, usize, usize) {
        //     for lvl in (0..M::PAGE_TABLE_LEVELS).rev() {
        //         let page_size = 8 << (M::PAGE_ENTRY_SHIFT * (lvl + 1));
        //         if for_range.size() % page_size == 0 {
        //             return (lvl, page_size, for_range.size() / page_size);
        //         }
        //     }
        //     unreachable!()
        // };
        //
        // let (lvl, page_size, len_pages) = find_lvl(&virt);
        // for i in 0..len_pages {
        //     f(i, lvl, page_size)?;
        // }

        let lvl = 0;
        let len_pages = virt.size() / M::PAGE_SIZE;

        for i in 0..len_pages {
            f(i, lvl, M::PAGE_SIZE)?;
        }

        // let mut rest = virt.size();
        // for lvl in (0..M::PAGE_TABLE_LEVELS).rev() {
        //     let page_size = 8 << (M::PAGE_ENTRY_SHIFT * (lvl + 1));
        //     rest = rest % page_size;
        //     let len_pages = rest / page_size;
        //     log::trace!("Processing {len_pages} pages at lvl {lvl}, rest bytes {rest}");
        //
        //     for i in 0..len_pages {
        //         f(i, lvl, page_size)?;
        //     }
        // }

        Ok(())
    }
}

// bitflags::bitflags! {
//     struct ElfShFlags: u64 {
//         /// Section is writable.
//         const SHF_WRITE = 1 << 0;
//         /// Section occupies memory during execution.
//         const SHF_ALLOC = 1 << 1;
//         /// Section is executable.
//         const SHF_EXECINSTR = 1 << 2;
//         /// Section holds thread-local storage.
//         const SHF_TLS = 1 << 10;
//     }
// }
//
// impl<'a, M: Mode> Mapper<'a, M> {
//     pub fn map_elf(
//         &mut self,
//         obj: &object::File,
//         flush: &mut Flush<M>,
//         flags_for_section: impl Fn(&Section) -> object::read::Result<M::EntryFlags>,
//     ) -> crate::Result<()> {
//         debug_assert!(matches!(obj.format(), BinaryFormat::Elf));
//
//         let regions: MemoryRegions<M, _, _> =
//             MemoryRegions::new(obj.sections(), flags_for_section);
//
//         for region in regions {
//             assert!(region.range_virt.is_aligned(M::PAGE_SIZE));
//
//             let size_pages = region.range_virt.size() / M::PAGE_SIZE;
//
//             let range_phys = {
//                 let start = self.allocator.allocate_frames(size_pages)?;
//                 start..start.add(size_pages * M::PAGE_SIZE)
//             };
//
//             log::trace!(
//                 "Mapping {:?} sections of ELF {:?} => {range_phys:?}, flags {:?}",
//                 region.sections,
//                 region.range_virt,
//                 region.flags
//             );
//             self.map_range(region.range_virt, range_phys, region.flags, flush)?;
//         }
//
//         Ok(())
//     }
// }
//
// struct Buffer<T, const N: usize> {
//     inner: [MaybeUninit<T>; N],
//     len: usize,
// }
//
// impl<T, const N: usize> Buffer<T, N> {
//     pub fn push(&mut self, v: T) {
//         assert!(self.len < N);
//
//         self.inner[self.len - 1].write(v);
//         self.len += 1;
//     }
//
//     pub fn as_slice(&self) -> &[T] {
//         unsafe {
//             mem::transmute(&self.inner[0..self.len])
//         }
//     }
// }
//
// impl<T, const N: usize> Default for Buffer<T, N> {
//     fn default() -> Self {
//         Self {
//             inner: unsafe { MaybeUninit::uninit().assume_init() },
//             len: 0,
//         }
//     }
// }
//
// impl<T: fmt::Debug, const N: usize> fmt::Debug for Buffer<T, N> {
//     fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
//         f.debug_list().entries(self.as_slice()).finish()
//     }
// }
//
// struct MemoryRegions<'file, 'data, M: Mode, I, F> {
//     buffer: Buffer<Section<'file, 'data>, 16>,
//     range_virt: Range<VirtualAddress>,
//     flags: M::EntryFlags,
//     inner: I,
//     flags_for_section: F,
// }
//
// struct MemoryRegion<'file, 'data, M: Mode> {
//     sections: Buffer<Section<'file, 'data>, 16>,
//     range_virt: Range<VirtualAddress>,
//     flags: M::EntryFlags,
// }
//
// impl<'file, 'data, M: Mode, I, F> MemoryRegions<'file, 'data, M, I, F> {
//     pub fn new(inner: I, flags_for_section: F) -> Self {
//         Self {
//             buffer: Buffer::default(),
//             range_virt: Default::default(),
//             flags: M::EntryFlags::empty(),
//             inner,
//             flags_for_section,
//         }
//     }
// }
//
// impl<'file: 'data, 'data, M, I, F> Iterator for MemoryRegions<'file, 'data, M, I, F>
// where
//     M: Mode,
//     I: Iterator<Item = Section<'file, 'data>>,
//     F: Fn(&Section) -> object::read::Result<M::EntryFlags>,
// {
//     type Item = MemoryRegion<'file, 'data, M>;
//
//     fn next(&mut self) -> Option<Self::Item> {
//         loop {
//             let section = self.inner.next()?;
//             // skip sections that are marked as non-alloc
//             if !is_alloc(&section) {
//                 continue;
//             }
//
//             let flags = (self.flags_for_section)(&section).ok()?;
//
//             let range_virt = unsafe {
//                 let start = VirtualAddress::new(section.address() as usize);
//
//                 start..start.add(section.size() as usize)
//             };
//
//             if flags.contains(self.flags) {
//                 self.range_virt = Range {
//                     start: cmp::min(self.range_virt.start, range_virt.start),
//                     end: cmp::max(self.range_virt.start, range_virt.end),
//                 };
//             } else {
//                 let range_virt = mem::replace(&mut self.range_virt, range_virt);
//                 let flags = mem::replace(&mut self.flags, flags);
//                 let sections = mem::take(&mut self.buffer);
//                 self.buffer.push(section);
//
//                 break Some(MemoryRegion {
//                     range_virt,
//                     flags,
//                     sections,
//                 });
//             }
//         }
//     }
// }
//
// /// Whether a given section occupies memory during execution.
// fn is_alloc(section: &Section) -> bool {
//     const SHF_ALLOC: u64 = 1 << 1;
//
//     match section.flags() {
//         SectionFlags::Elf { sh_flags } => sh_flags & SHF_ALLOC != 0,
//         _ => unreachable!(),
//     }
// }
