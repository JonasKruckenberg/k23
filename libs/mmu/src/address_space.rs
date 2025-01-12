// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::frame_alloc::{FrameAllocator, FramesIterator};
use crate::{arch, ensure, Error, Flush, PhysicalAddress, VirtualAddress};
use core::alloc::Layout;
use core::fmt;
use core::fmt::Formatter;
use core::num::NonZeroUsize;
use core::ptr::NonNull;
use core::range::Range;

const SINGLE_FRAME_LAYOUT: Layout =
    unsafe { Layout::from_size_align_unchecked(arch::PAGE_SIZE, arch::PAGE_SIZE) };

pub struct AddressSpace {
    root_pgtable: PhysicalAddress,
    phys_offset: VirtualAddress,
    asid: usize,
}

impl AddressSpace {
    /// Create a new address space with a fresh hardware page table.
    pub fn new(
        frame_alloc: &mut dyn FrameAllocator,
        asid: usize,
        phys_offset: VirtualAddress,
    ) -> crate::Result<(Self, Flush)> {
        let root_pgtable = frame_alloc
            .allocate_contiguous_zeroed(SINGLE_FRAME_LAYOUT)
            .ok_or(Error::NoMemory)?; // we should be able to map a single page

        let this = Self {
            asid,
            phys_offset,
            root_pgtable,
        };

        Ok((this, Flush::empty(asid)))
    }

    /// Create an address space from the currently active hardware page table.
    pub fn from_active(asid: usize, phys_offset: VirtualAddress) -> (Self, Flush) {
        let root_pgtable = arch::get_active_pgtable(asid);
        debug_assert!(root_pgtable.get() != 0);

        let this = Self {
            asid,
            phys_offset,
            root_pgtable,
        };

        (this, Flush::empty(asid))
    }

    /// Return the physical address of the hardware page table root.
    pub fn root_pgtable(&self) -> PhysicalAddress {
        self.root_pgtable
    }

    /// Return the offset of the physical memory mapping in this address space.
    pub fn physical_memory_offset(&self) -> VirtualAddress {
        self.phys_offset
    }

    /// Convert a physical address to a virtual address in this address space
    pub fn phys_to_virt(&self, phys: PhysicalAddress) -> VirtualAddress {
        self.phys_offset.checked_add(phys.get()).unwrap()
    }

    /// Return the address space identifier (ASID) of this address space
    pub fn asid(&self) -> usize {
        self.asid
    }

    /// Map an iterator of possibly non-contiguous physical frames into virtual memory starting
    /// at `virt`. The size of the mapping is determined by the sum of the sizes of the frame regions.
    ///
    /// Note that this function expects the virtual memory to be unmapped, to remap an already mapped
    /// region use [`self.remap`].
    ///
    /// # Safety
    ///
    /// This method **does not** validate invariants upfront for performance reasons. This means
    /// a panic or error in this method might leave the address space in an invalid state. It is
    /// up to the caller to deal with this (most likely you want to just discard the address space).
    ///
    /// # Panics
    ///
    /// This method will panic if the following preconditions aren't met:
    /// - `virt + mapped len` must not overflow
    /// - the entire range must be unmapped
    ///
    /// With debug assertions enabled the following additional invariants are also enforced:
    /// - `virt` must at least be page aligned
    /// - physical addresses produced by `frames` must be at least page aligned
    /// - the total size produced by `frames` must be an integer multiple of the page size
    pub unsafe fn map(
        &mut self,
        mut virt: VirtualAddress,
        mut frames: impl FramesIterator,
        flags: crate::Flags,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        while let Some((phys, len)) = frames.next() {
            unsafe {
                self.map_contiguous(
                    frames.alloc_mut(),
                    virt,
                    phys,
                    NonZeroUsize::new(len).unwrap(),
                    flags,
                    flush,
                )?;
            }
            virt = virt.checked_add(len).unwrap();
        }

        Ok(())
    }

    /// Map a contiguous run of physical frames of length `len` into virtual memory at `virt`.
    ///
    /// Unless you absolutely require the physical frames to be contiguous, you should use [`self.map`]
    /// instead.
    ///
    /// Note that this function expects the virtual memory to be unmapped, to remap an already mapped
    /// region use [`self.remap_contiguous`].
    ///
    /// # Safety
    ///
    /// This method **does not** validate invariants upfront for performance reasons. This means
    /// a panic or error in this method might leave the address space in an invalid state. It is
    /// up to the caller to deal with this (most likely you want to just discard the address space).
    ///
    /// # Panics
    ///
    /// This method will panic if the following preconditions aren't met:
    /// - `virt + mapped len` must not overflow
    /// - the entire range must be unmapped
    ///
    /// With debug assertions enabled the following additional invariants are also enforced:
    /// - `virt` must be at least page aligned
    /// - `phys` must be at least page aligned
    /// - `len` must be an integer multiple of the page size
    pub unsafe fn map_contiguous(
        &mut self,
        frame_alloc: &mut dyn FrameAllocator,
        mut virt: VirtualAddress,
        mut phys: PhysicalAddress,
        len: NonZeroUsize,
        flags: crate::Flags,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        let mut remaining_bytes = len.get();
        debug_assert!(
            remaining_bytes >= arch::PAGE_SIZE,
            "address range span be at least one page"
        );
        debug_assert!(
            virt.is_aligned_to(arch::PAGE_SIZE),
            "virtual address must be aligned to at least 4KiB page size {virt:?}"
        );
        debug_assert!(
            phys.is_aligned_to(arch::PAGE_SIZE),
            "physical address must be aligned to at least 4KiB page size {phys:?}"
        );

        // To map out contiguous chunk of physical memory into the virtual address space efficiently
        // we'll attempt to map as much of the chunk using as large of a page size as possible.
        //
        // We'll follow the page table down starting at the root page table entry (PTE) and check at
        // every level if we can map there. This is dictated by the combination of virtual and
        // physical address alignment as well as chunk size. If we can map at the current level
        // well subtract the page size from `remaining_bytes`, advance the current virtual and physical
        // addresses by the page size and repeat the process until there are no more bytes to map.
        //
        // IF we can't map at a given level, we'll either allocate a new PTE or follow and existing PTE
        // to the next level (and therefore smaller page size) until we reach a level that we can map at.
        // Note that, because we require a minimum alignment and size of PAGE_SIZE, we will always be
        // able to map a chunk using level 0 pages.
        //
        // In effect this algorithm will map the start and ends of chunks using smaller page sizes
        // while mapping the vast majority of the middle of a chunk using larger page sizes.
        'outer: while remaining_bytes > 0 {
            let mut pgtable: NonNull<arch::PageTableEntry> =
                self.pgtable_ptr_from_phys(self.root_pgtable);

            for lvl in (0..arch::PAGE_TABLE_LEVELS).rev() {
                let index = arch::pte_index_for_level(virt, lvl);
                let pte = unsafe { pgtable.add(index).as_mut() };

                if !pte.is_valid() {
                    // If the PTE is invalid that means we reached a vacant slot to map into.
                    //
                    // First, lets check if we can map at this level of the page table given our
                    // current virtual and physical address as well as the number of remaining bytes.
                    if arch::can_map_at_level(virt, phys, remaining_bytes, lvl) {
                        let page_size = arch::page_size_for_level(lvl);

                        // This PTE is vacant AND we can map at this level
                        // mark this PTE as a valid leaf node pointing to the physical frame
                        pte.replace_address_and_flags(
                            phys,
                            arch::PTE_FLAGS_VALID.union(flags.into()),
                        );

                        flush.extend_range(
                            self.asid,
                            Range::from(virt..virt.checked_add(page_size).unwrap()),
                        )?;
                        virt = virt.checked_add(page_size).unwrap();
                        phys = phys.checked_add(page_size).unwrap();
                        remaining_bytes -= page_size;
                        continue 'outer;
                    } else if !pte.is_valid() {
                        // The current PTE is vacant, but we couldn't map at this level (because the
                        // page size was too large, or the request wasn't sufficiently aligned or
                        // because the architecture just can't map at this level). This means
                        // we need to allocate a new sub-table and retry.
                        // allocate a new physical frame to hold the next level table and
                        // mark this PTE as a valid internal node pointing to that sub-table.
                        let frame = frame_alloc
                            .allocate_contiguous_zeroed(SINGLE_FRAME_LAYOUT)
                            .ok_or(Error::NoMemory)?; // we should always be able to map a single page

                        // TODO memory barrier

                        pte.replace_address_and_flags(frame, arch::PTE_FLAGS_VALID);

                        pgtable = self.pgtable_ptr_from_phys(frame);
                    }
                } else if !pte.is_leaf() {
                    // This PTE is an internal node pointing to another page table
                    pgtable = self.pgtable_ptr_from_phys(pte.get_address_and_flags().0);
                } else {
                    unreachable!("Invalid state: PTE can't be valid leaf (this means {virt:?} is already mapped) {pte:?} {pte:p}");
                }
            }
        }

        Ok(())
    }

    /// Remap an iterator of possibly non-contiguous physical frames into virtual memory starting
    /// at `virt`. The size of the mapping is determined by the sum of the sizes of the frame regions.
    ///
    /// This function is similar to [`self.map`], but it expects the virtual memory to be already mapped
    /// and simply changes the address of the backing frames.
    ///
    /// # Safety
    ///
    /// This method **does not** validate invariants upfront for performance reasons. This means
    /// a panic or error in this method might leave the address space in an invalid state. It is
    /// up to the caller to deal with this (most likely you want to just discard the address space).
    ///
    /// # Panics
    ///
    /// This method will panic if the following preconditions aren't met:
    /// - `virt + mapped len` must not overflow
    /// - the entire range must be mapped
    /// - the alignment of `virt`, and `phys` addresses produced by `frames` must be the same as the
    ///     existing mappings. That means when the mapping was initially created with a 2MiB alignment
    ///     `remap` must also be called with 2MiB aligned addresses.
    ///
    /// With debug assertions enabled the following additional invariants are also enforced:
    /// - `virt` must at least be page aligned
    /// - physical addresses produced by `frames` must be at least page aligned
    /// - the total size produced by `frames` must be an integer multiple of the page size
    pub unsafe fn remap(
        &mut self,
        mut virt: VirtualAddress,
        frames: impl FramesIterator,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        for (phys, len) in frames {
            unsafe {
                self.remap_contiguous(virt, phys, NonZeroUsize::new(len).unwrap(), flush)?;
            }
            virt = virt.checked_add(len).unwrap();
        }

        Ok(())
    }

    /// Remap a contiguous run of physical frames of length `len` into virtual memory at `virt`.
    ///
    /// Unless you absolutely require the physical frames to be contiguous, you should use [`self.remap`]
    /// instead.
    ///
    /// This function is similar to [`self.map_contiguous`], but it expects the virtual memory to be already mapped
    /// and simply changes the address of the backing frames.
    ///
    /// # Safety
    ///
    /// This method **does not** validate invariants upfront for performance reasons. This means
    /// a panic or error in this method might leave the address space in an invalid state. It is
    /// up to the caller to deal with this (most likely you want to just discard the address space).
    ///
    /// # Panics
    ///
    /// This method will panic if the following preconditions aren't met:
    /// - `virt + mapped len` must not overflow
    /// - the entire range must be mapped
    /// - the alignment of `virt`, and `phys` must be the same as the existing mappings. That means
    ///     when the mapping was initially created with a 2MiB alignment `remap` must also be called
    ///     with 2MiB aligned addresses.
    ///
    /// With debug assertions enabled the following additional invariants are also enforced:
    /// - `virt` must be at least page aligned
    /// - `phys` must be at least page aligned
    /// - `len` must be an integer multiple of the page size
    pub unsafe fn remap_contiguous(
        &mut self,
        mut virt: VirtualAddress,
        mut phys: PhysicalAddress,
        len: NonZeroUsize,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        let mut remaining_bytes = len.get();
        debug_assert!(
            remaining_bytes >= arch::PAGE_SIZE,
            "virtual address range must span be at least one page"
        );
        debug_assert!(
            virt.is_aligned_to(arch::PAGE_SIZE),
            "virtual address must be aligned to at least 4KiB page size"
        );
        debug_assert!(
            phys.is_aligned_to(arch::PAGE_SIZE),
            "physical address must be aligned to at least 4KiB page size"
        );

        // This algorithm behaves a lot like the above one for `map_contiguous` but with an important
        // distinction: Since we require the entire range to already be mapped, we follow the page tables
        // until we reach a valid PTE. Once we reached, we assert that we can map the given physical
        // address here and simply update the PTEs address. We then again repeat this until we have
        // no more bytes to process.
        'outer: while remaining_bytes > 0 {
            let mut pgtable = self.pgtable_ptr_from_phys(self.root_pgtable);

            for lvl in (0..arch::PAGE_TABLE_LEVELS).rev() {
                let pte = unsafe {
                    let index = arch::pte_index_for_level(virt, lvl);
                    pgtable.add(index).as_mut()
                };

                if pte.is_valid() && pte.is_leaf() {
                    // We reached the previously mapped leaf node that we want to edit
                    // assert that we can actually map at this level (remap requires users to remap
                    // only to equal or larger alignments, but we should make sure.
                    let page_size = arch::page_size_for_level(lvl);

                    debug_assert!(
                        arch::can_map_at_level(virt, phys, remaining_bytes, lvl),
                        "remapping requires the same alignment ({page_size}) but found {phys:?}, {remaining_bytes}bytes"
                    );

                    let (_old_phys, flags) = pte.get_address_and_flags();
                    pte.replace_address_and_flags(phys, flags);

                    flush.extend_range(
                        self.asid,
                        Range::from(virt..virt.checked_add(page_size).unwrap()),
                    )?;
                    virt = virt.checked_add(page_size).unwrap();
                    phys = phys.checked_add(page_size).unwrap();
                    remaining_bytes -= page_size;
                    continue 'outer;
                } else if pte.is_valid() {
                    // This PTE is an internal node pointing to another page table
                    pgtable = self.pgtable_ptr_from_phys(pte.get_address_and_flags().0);
                } else {
                    unreachable!("Invalid state: PTE cant be vacant or invalid+leaf {pte:?}");
                }
            }
        }

        Ok(())
    }

    /// Change the permissions of a range of virtual memory starting at `virt` and of length
    /// `len` to `new_flags`.
    ///
    /// Note that you can only **reduce** permissions with this function, never increase them.
    ///
    /// # Safety
    ///
    /// This method **does not** validate invariants upfront for performance reasons. This means
    /// a panic or error in this method might leave the address space in an invalid state. It is
    /// up to the caller to deal with this (most likely you want to just discard the address space).
    ///
    /// # Panics
    ///
    /// This method will panic if the following preconditions aren't met:
    /// - `virt + mapped len` must not overflow
    /// - the entire range must be mapped
    ///
    /// With debug assertions enabled the following additional invariants are also enforced:
    /// - `virt` must be at least page aligned
    /// - `phys` must be at least page aligned
    /// - `len` must be an integer multiple of the page size
    pub unsafe fn protect(
        &mut self,
        mut virt: VirtualAddress,
        len: NonZeroUsize,
        new_flags: crate::Flags,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        let mut remaining_bytes = len.get();
        debug_assert!(
            remaining_bytes >= arch::PAGE_SIZE,
            "virtual address range must span be at least one page"
        );
        debug_assert!(
            virt.is_aligned_to(arch::PAGE_SIZE),
            "virtual address must be aligned to at least 4KiB page size"
        );

        // The algorithm below is essentially the same as `remap_contiguous` but with the difference
        // that we don't replace the PTEs address but instead the PTEs flags ensuring that the caller
        // can't increase the permissions.
        'outer: while remaining_bytes > 0 {
            let mut pgtable = self.pgtable_ptr_from_phys(self.root_pgtable);

            for lvl in (0..arch::PAGE_TABLE_LEVELS).rev() {
                let pte = unsafe {
                    let index = arch::pte_index_for_level(virt, lvl);
                    pgtable.add(index).as_mut()
                };

                if pte.is_valid() && pte.is_leaf() {
                    // We reached the previously mapped leaf node that we want to edit
                    // firstly, ensure that this operation only removes permissions never adds any
                    // and secondly mask out the old permissions replacing them with the new. This must
                    // ensure we retain any other flags in the process.
                    let rwx_mask = arch::PTE_FLAGS_RWX_MASK;

                    let new_flags = rwx_mask.intersection(new_flags.into());
                    let (phys, old_flags) = pte.get_address_and_flags();

                    ensure!(
                        old_flags.intersection(rwx_mask).contains(new_flags),
                        Error::PermissionIncrease
                    );

                    pte.replace_address_and_flags(
                        phys,
                        old_flags.difference(rwx_mask).union(new_flags),
                    );

                    let page_size = arch::page_size_for_level(lvl);
                    flush.extend_range(
                        self.asid,
                        Range::from(virt..virt.checked_add(page_size).unwrap()),
                    )?;
                    virt = virt.checked_add(page_size).unwrap();
                    remaining_bytes -= page_size;
                    continue 'outer;
                } else if pte.is_valid() {
                    // This PTE is an internal node pointing to another page table
                    pgtable = self.pgtable_ptr_from_phys(pte.get_address_and_flags().0);
                } else {
                    unreachable!("Invalid state: PTE cant be vacant or invalid+leaf {pte:?}");
                }
            }
        }

        Ok(())
    }

    /// Unmap a range of virtual memory starting at `virt` and of length `len`, this will also
    /// free the physical frames backing the virtual memory and page table entries.
    ///
    /// # Safety
    ///
    /// This method **does not** validate invariants upfront for performance reasons. This means
    /// a panic or error in this method might leave the address space in an invalid state. It is
    /// up to the caller to deal with this (most likely you want to just discard the address space).
    ///
    /// # Panics
    ///
    /// This method will panic if the following preconditions aren't met:
    /// - `virt + len` must not overflow
    /// - the entire range must be mapped
    pub unsafe fn unmap(
        &mut self,
        frame_alloc: &mut dyn FrameAllocator,
        mut virt: VirtualAddress,
        len: NonZeroUsize,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        let mut remaining_bytes = len.get();
        debug_assert!(
            remaining_bytes >= arch::PAGE_SIZE,
            "virtual address range must span be at least one page"
        );
        debug_assert!(
            virt.is_aligned_to(arch::PAGE_SIZE),
            "virtual address must be aligned to at least 4KiB page size"
        );

        // The algorithm of this function is different from the others, it processes the requested range
        // in an iterative fashion, but will not traverse the page table levels iteratively instead
        // doing so recursively. The reason for this is that after unmapping a page we need to check
        // if by doing so the parent has become empty in which case we also need to unmap the parent.
        while remaining_bytes > 0 {
            unsafe {
                self.unmap_inner(
                    self.pgtable_ptr_from_phys(self.root_pgtable),
                    frame_alloc,
                    &mut virt,
                    &mut remaining_bytes,
                    arch::PAGE_TABLE_LEVELS - 1,
                    flush,
                )?;
            }
        }

        Ok(())
    }

    unsafe fn unmap_inner(
        &mut self,
        pgtable: NonNull<arch::PageTableEntry>,
        frame_alloc: &mut dyn FrameAllocator,
        virt: &mut VirtualAddress,
        remaining_bytes: &mut usize,
        lvl: usize,
        flush: &mut Flush,
    ) -> crate::Result<()> {
        let index = arch::pte_index_for_level(*virt, lvl);
        let pte = unsafe { pgtable.add(index).as_mut() };
        let page_size = arch::page_size_for_level(lvl);

        if pte.is_valid() && pte.is_leaf() {
            // The PTE is mapped, so go ahead and unmap it giving back its
            // corresponding frame of memory to the allocator.

            let frame = pte.get_address_and_flags().0;

            frame_alloc.deallocate_contiguous(
                frame,
                Layout::from_size_align(page_size, page_size).unwrap(),
            );
            pte.clear();

            flush.extend_range(
                self.asid,
                Range::from(*virt..virt.checked_add(page_size).unwrap()),
            )?;
            *virt = virt.checked_add(page_size).unwrap();
            *remaining_bytes -= page_size;
        } else if pte.is_valid() {
            // This PTE is an internal node pointing to another page table
            let pgtable = self.pgtable_ptr_from_phys(pte.get_address_and_flags().0);
            unsafe {
                self.unmap_inner(pgtable, frame_alloc, virt, remaining_bytes, lvl - 1, flush)?;
            }

            // The recursive descend above might have unmapped the last child of this PTE in which
            // case we need to unmap it as well

            // TODO optimize this
            let is_still_populated = (0..arch::PAGE_TABLE_ENTRIES)
                .any(|index| unsafe { pgtable.add(index).as_ref().is_valid() });

            if !is_still_populated {
                let frame = pte.get_address_and_flags().0;
                frame_alloc.deallocate_contiguous(frame, SINGLE_FRAME_LAYOUT);
                pte.clear();
            }
        } else {
            unreachable!("Invalid state: PTE cant be invalid (this means {virt:?} is already unmapped) {pte:?}");
        }

        Ok(())
    }

    /// Resolve a virtual address to its backing physical address and associated page table entry flags.
    pub fn query(&mut self, virt: VirtualAddress) -> Option<(PhysicalAddress, crate::Flags)> {
        let mut pgtable: NonNull<arch::PageTableEntry> =
            self.pgtable_ptr_from_phys(self.root_pgtable);

        for lvl in (0..arch::PAGE_TABLE_LEVELS).rev() {
            let pte = unsafe {
                let index = arch::pte_index_for_level(virt, lvl);
                pgtable.add(index).as_mut()
            };

            if pte.is_valid() && pte.is_leaf() {
                let (addr, flags) = pte.get_address_and_flags();
                return Some((addr, flags.into()));
            } else if pte.is_valid() {
                // This PTE is an internal node pointing to another page table
                pgtable = self.pgtable_ptr_from_phys(pte.get_address_and_flags().0);
            } else {
                // This PTE is vacant, which means at whatever level we're at, there is no
                // point at doing any more work since this address cannot be mapped to anything
                // anyway.

                return None;
            }
        }

        None
    }

    /// Activate this address space
    ///
    /// # Safety
    ///
    /// This will invalidate pointers if not used carefully
    pub unsafe fn activate(&self) {
        unsafe { arch::activate_pgtable(self.asid, self.root_pgtable) }
    }

    fn pgtable_ptr_from_phys(&self, phys: PhysicalAddress) -> NonNull<arch::PageTableEntry> {
        NonNull::new(
            self.phys_offset
                .checked_add(phys.get())
                .unwrap()
                .as_mut_ptr()
                .cast(),
        )
        .unwrap()
    }
}

impl fmt::Display for AddressSpace {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        fn fmt_table(
            f: &mut Formatter<'_>,
            aspace: &AddressSpace,
            pgtable: NonNull<arch::PageTableEntry>,
            acc: VirtualAddress,
            lvl: usize,
        ) -> fmt::Result {
            let padding = match lvl {
                0 => 8,
                1 => 4,
                _ => 0,
            };

            for index in 0..arch::PAGE_TABLE_ENTRIES {
                let pte = unsafe { pgtable.add(index).as_mut() };
                let virt = VirtualAddress(acc.get() | virt_from_index(lvl, index).get());
                let (address, flags) = pte.get_address_and_flags();

                if pte.is_valid() && pte.is_leaf() {
                    writeln!(
                        f,
                        "{:^padding$}{}:{index:<3} is a leaf {} => {} {:?}",
                        "", lvl, virt, address, flags
                    )?;
                } else if pte.is_valid() {
                    writeln!(f, "{:^padding$}{}:{index} is a table node", "", lvl)?;
                    let (address, _) = pte.get_address_and_flags();
                    let pgtable = aspace.pgtable_ptr_from_phys(address);
                    fmt_table(f, aspace, pgtable, virt, lvl - 1)?
                }
            }

            Ok(())
        }

        fmt_table(
            f,
            self,
            self.pgtable_ptr_from_phys(self.root_pgtable),
            VirtualAddress::default(),
            arch::PAGE_TABLE_LEVELS - 1,
        )
    }
}

#[allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]
fn virt_from_index(lvl: usize, index: usize) -> VirtualAddress {
    let raw = ((index & (arch::PAGE_TABLE_ENTRIES - 1))
        << (lvl * arch::PAGE_ENTRY_SHIFT + arch::PAGE_SHIFT)) as isize;

    let shift = size_of::<usize>() as u32 * 8 - (arch::VIRT_ADDR_BITS + 1);
    VirtualAddress(raw.wrapping_shl(shift).wrapping_shr(shift) as usize)
}
