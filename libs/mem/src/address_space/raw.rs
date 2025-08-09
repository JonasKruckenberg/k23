// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::num::NonZeroUsize;

use crate::{AccessRules, PhysicalAddress, VirtualAddress};

/// Low-level address space management, providing methods for mapping, unmapping,
/// and modifying access rules for contiguous virtual-to-physical memory regions.
///
/// ### Alignment
///
/// All addresses passed as arguments to methods of this trait must be aligned to `Self::PAGE_SIZE`.
///
/// ### Currently mapped memory
///
/// [`unmap`] and [`set_access_rules`] require that a memory region is *currently mapped* by an
/// address space. This means that:
/// - the region was previously mapped by `map`, and
/// - the region was not subsequently unmapped.
///
/// # Safety
///
/// Virtual memory ranges that are [*currently mapped*] by an address space, must map to valid
/// physical memory, and retain their validity until either:
/// - the mapping is removed through [`unmap`], or
/// - the address space is dropped
///
/// [*currently mapped*]: #currently-mapped-memory
/// [`map`]: Self::map
/// [`unmap`]: Self::unmap
pub unsafe trait RawAddressSpace {
    /// The smallest addressable chunk of memory of this address space. All address argument provided
    /// to methods of this type (both virtual and physical) must be aligned to this.
    const PAGE_SIZE: NonZeroUsize;
    const PAGE_SIZE_LOG_2: u8 = (Self::PAGE_SIZE.get() - 1).count_ones() as u8;

    /// The [`Flush`] implementation for this address space.
    type Flush: Flush;

    /// Return a new, empty flush for this address space.
    fn flush(&self) -> Self::Flush;

    /// Return the corresponding [`PhysicalAddress`] and [`AccessRules`] for the given
    /// [`VirtualAddress`] if mapped.
    fn lookup(&self, virt: VirtualAddress) -> Option<(PhysicalAddress, AccessRules)>;

    /// Map a contiguous range of `len` virtual addresses to `len` physical addresses with the
    /// specified access rules.
    ///
    /// If this returns `Ok`, the mapping is added to the raw address space and all future
    /// accesses to the virtual address range will translate to accesses of the physical address
    /// range.
    ///
    /// # Safety
    ///
    /// - `virt` must be aligned to `Self::PAGE_SIZE`
    /// - `phys` must be aligned to `Self::PAGE_SIZE`
    /// - `len` must an integer multiple of `Self::PAGE_SIZE`
    ///
    /// # Errors
    ///
    /// Returning `Err` indicates the mapping cannot be established and the virtual address range
    /// remains unaltered.
    unsafe fn map(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        len: NonZeroUsize,
        access_rules: AccessRules,
        flush: &mut Self::Flush,
    ) -> crate::Result<()>;

    /// Unmap a contiguous range of `len` virtual addresses.
    ///
    /// After this returns all accesses to the virtual address region will cause a fault.
    ///
    /// # Safety
    ///
    /// - `virt..virt+len` must be mapped
    /// - `virt` must be aligned to `Self::PAGE_SIZE`
    /// - `phys` must be aligned to `Self::PAGE_SIZE`
    /// - `len` must an integer multiple of `Self::PAGE_SIZE`
    unsafe fn unmap(&mut self, virt: VirtualAddress, len: NonZeroUsize, flush: &mut Self::Flush);

    /// Set the [`AccessRules`] for a contiguous range of `len` virtual addresses.
    ///
    /// After this returns all accesses to the virtual address region must follow the
    /// specified `AccessRules` or cause a fault.
    ///
    /// # Safety
    ///
    /// - `virt..virt+len` must be mapped
    /// - `virt` must be aligned to `Self::PAGE_SIZE`
    /// - `phys` must be aligned to `Self::PAGE_SIZE`
    /// - `len` must an integer multiple of `Self::PAGE_SIZE`
    unsafe fn set_access_rules(
        &mut self,
        virt: VirtualAddress,
        len: NonZeroUsize,
        access_rules: AccessRules,
        flush: &mut Self::Flush,
    );
}

/// A type that can flush changes made to a [`RawAddressSpace`].
///
/// Note: [`Flush`] is purely optional, it exists so implementation MAY batch
/// Note that the implementation is not required to delay materializing changes until [`Flush::flush`]
/// is called.
pub trait Flush {
    /// Flush changes made to its [`RawAddressSpace`].
    ///
    /// If this returns `Ok`, changes made to the address space are REQUIRED to take effect across
    /// all affected threads/CPUs.
    ///
    /// # Errors
    ///
    /// If this returns `Err`, if flushing the changes failed. The changes, or a subset of them, might
    /// still have taken effect across all or some of the threads/CPUs.
    fn flush(self) -> crate::Result<()>;
}
