// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::cmp;
use core::num::{NonZero, NonZeroUsize};

use smallvec::SmallVec;

use crate::address_space::{Flush, RawAddressSpace};
use crate::{AccessRules, PhysicalAddress, VirtualAddress};

/// [`Batch`] maintains an *unordered* set of batched operations over an `RawAddressSpace`.
///
/// Operations are "enqueued" (but unordered) into the batch and executed against the raw address space
/// when [`Self::flush_changes`] is called. This helps to reduce the number and size of (expensive) TLB
/// flushes we need to perform. Internally, `Batch` will merge operations if possible to further reduce
/// this number.
pub struct Batch {
    ops: SmallVec<[BatchOperation; 4]>,
}

enum BatchOperation {
    Map(MapOperation),
    Unmap(UnmapOperation),
    SetAccessRules(SetAccessRulesOperation),
}

struct MapOperation {
    virt: VirtualAddress,
    phys: PhysicalAddress,
    len: NonZeroUsize,
    access_rules: AccessRules,
}

struct UnmapOperation {
    virt: VirtualAddress,
    len: NonZeroUsize,
}

struct SetAccessRulesOperation {
    virt: VirtualAddress,
    len: NonZeroUsize,
    access_rules: AccessRules,
}

// ===== impl Batch =====

impl Batch {
    /// Construct a new empty [`Batch`].
    pub fn new() -> Self {
        Self {
            ops: SmallVec::new(),
        }
    }

    /// Add a [`map`] operation to the set of batched operations.
    ///
    /// # Safety
    ///
    /// - `virt` must be aligned to `Self::PAGE_SIZE`
    /// - `phys` must be aligned to `Self::PAGE_SIZE`
    /// - `len` must an integer multiple of `Self::PAGE_SIZE`
    ///
    /// [`map`]: RawAddressSpace::map
    pub unsafe fn map(
        &mut self,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        len: NonZeroUsize,
        access_rules: AccessRules,
    ) {
        let mut new = MapOperation {
            virt,
            phys,
            len,
            access_rules,
        };

        let ops = self.ops.iter_mut().filter_map(|op| match op {
            BatchOperation::Map(op) => Some(op),
            _ => None,
        });

        for op in ops {
            match op.try_merge_with(new) {
                Ok(()) => return,
                Err(new_) => new = new_,
            }
        }

        self.ops.push(BatchOperation::Map(new));
    }

    /// Add an [`unmap`] operation to the set of batched operations.
    ///
    /// # Safety
    ///
    /// - virt..virt+len must be mapped
    /// - `virt` must be aligned to `Self::PAGE_SIZE`
    /// - `phys` must be aligned to `Self::PAGE_SIZE`
    /// - `len` must an integer multiple of `Self::PAGE_SIZE`
    ///
    /// [`unmap`]: RawAddressSpace::unmap
    pub unsafe fn unmap(&mut self, virt: VirtualAddress, len: NonZeroUsize) {
        let mut new = UnmapOperation { virt, len };

        let ops = self.ops.iter_mut().filter_map(|op| match op {
            BatchOperation::Unmap(op) => Some(op),
            _ => None,
        });

        for op in ops {
            match op.try_merge_with(new) {
                Ok(()) => return,
                Err(new_) => new = new_,
            }
        }

        self.ops.push(BatchOperation::Unmap(new));
    }

    /// Add a [`set_access_rules`] operation to the set of batched operations.
    ///
    /// # Safety
    ///
    /// - virt..virt+len must be mapped
    /// - `virt` must be aligned to `Self::PAGE_SIZE`
    /// - `phys` must be aligned to `Self::PAGE_SIZE`
    /// - `len` must an integer multiple of `Self::PAGE_SIZE`
    ///
    /// [`set_access_rules`]: RawAddressSpace::set_access_rules
    pub fn set_access_rules(
        &mut self,
        virt: VirtualAddress,
        len: NonZeroUsize,
        access_rules: AccessRules,
    ) {
        let mut new = SetAccessRulesOperation {
            virt,
            len,
            access_rules,
        };

        let ops = self.ops.iter_mut().filter_map(|op| match op {
            BatchOperation::SetAccessRules(op) => Some(op),
            _ => None,
        });

        for op in ops {
            match op.try_merge_with(new) {
                Ok(()) => return,
                Err(new_) => new = new_,
            }
        }

        self.ops.push(BatchOperation::SetAccessRules(new));
    }

    /// Flushes the `Batch` ensuring all changes are materialized into the raw address space.
    pub fn flush_changes<A: RawAddressSpace>(&mut self, raw_aspace: &mut A) -> crate::Result<()> {
        let mut flush = raw_aspace.flush();
        for op in self.ops.drain(..) {
            match op {
                BatchOperation::Map(op) => {
                    debug_assert!(op.virt.is_aligned_to(A::PAGE_SIZE));
                    debug_assert!(op.phys.is_aligned_to(A::PAGE_SIZE));
                    debug_assert!(op.len.get().is_multiple_of(A::PAGE_SIZE));

                    // Safety: the caller promised the correctness of the values on construction of
                    // the operation.
                    unsafe {
                        raw_aspace.map(op.virt, op.phys, op.len, op.access_rules, &mut flush)?;
                    }
                }
                BatchOperation::Unmap(op) => {
                    debug_assert!(op.virt.is_aligned_to(A::PAGE_SIZE));
                    debug_assert!(op.len.get().is_multiple_of(A::PAGE_SIZE));

                    // Safety: the caller promised the correctness of the values on construction of
                    // the operation.
                    unsafe {
                        raw_aspace.unmap(op.virt, op.len, &mut flush);
                    }
                }
                BatchOperation::SetAccessRules(op) => {
                    debug_assert!(op.virt.is_aligned_to(A::PAGE_SIZE));
                    debug_assert!(op.len.get().is_multiple_of(A::PAGE_SIZE));

                    // Safety: the caller promised the correctness of the values on construction of
                    // the operation.
                    unsafe {
                        raw_aspace.set_access_rules(op.virt, op.len, op.access_rules, &mut flush);
                    }
                }
            };
        }
        flush.flush()
    }
}

// ===== impl MapOperation =====

impl MapOperation {
    /// Returns true if this operation can be merged with `other`.
    ///
    /// Map operations can be merged if:
    /// - their [`AccessRules`] are the same
    /// - their virtual address ranges are contiguous (no gap between self and other)
    /// - their physical address ranges are contiguous
    /// - the resulting virtual address range still has the same size as the resulting
    ///   physical address range
    const fn can_merge_with(&self, other: &Self) -> bool {
        // the access rules need to be the same
        let same_rules = self.access_rules.bits() == other.access_rules.bits();

        let overlap_virt = self.virt.get() <= other.len.get()
            && other.virt.get() <= self.virt.get() + self.len.get();

        let overlap_phys = self.phys.get() <= other.len.get()
            && other.phys.get() <= self.phys.get() + self.len.get();

        let offset_virt = self.virt.get().wrapping_sub(other.virt.get());
        let offset_phys = self.virt.get().wrapping_sub(other.virt.get());
        let same_offset = offset_virt == offset_phys;

        same_rules && overlap_virt && overlap_phys && same_offset
    }

    /// Attempt to merge this operation with `other`.
    ///
    /// If this returns `Ok`, `other` has been merged into `self`.
    ///
    /// If this returns `Err`, `other` cannot be merged and is returned in the `Err` variant.
    fn try_merge_with(&mut self, other: Self) -> Result<(), Self> {
        if self.can_merge_with(&other) {
            let offset = self.virt.get().wrapping_sub(other.virt.get());
            let len = self
                .len
                .get()
                .checked_add(other.len.get())
                .unwrap()
                .wrapping_add(offset);

            self.virt = cmp::min(self.virt, other.virt);
            self.phys = cmp::min(self.phys, other.phys);
            self.len = NonZero::new(len).ok_or(other)?;

            Ok(())
        } else {
            Err(other)
        }
    }
}

// ===== impl UnmapOperation =====

impl UnmapOperation {
    /// Returns true if this operation can be merged with `other`.
    ///
    /// Unmap operations can be merged if:
    /// - their virtual address ranges are contiguous (no gap between self and other)
    const fn can_merge_with(&self, other: &Self) -> bool {
        self.virt.get() <= other.len.get() && other.virt.get() <= self.virt.get() + self.len.get()
    }

    /// Attempt to merge this operation with `other`.
    ///
    /// If this returns `Ok`, `other` has been merged into `self`.
    ///
    /// If this returns `Err`, `other` cannot be merged and is returned in the `Err` variant.
    fn try_merge_with(&mut self, other: Self) -> Result<(), Self> {
        if self.can_merge_with(&other) {
            let offset = self.virt.get().wrapping_sub(other.virt.get());
            let len = self
                .len
                .get()
                .checked_add(other.len.get())
                .unwrap()
                .wrapping_add(offset);

            self.virt = cmp::min(self.virt, other.virt);
            self.len = NonZero::new(len).ok_or(other)?;

            Ok(())
        } else {
            Err(other)
        }
    }
}

// ===== impl ProtectOperation =====

impl SetAccessRulesOperation {
    /// Returns true if this operation can be merged with `other`.
    ///
    /// Protect operations can be merged if:
    /// - their [`AccessRules`] are the same
    /// - their virtual address ranges are contiguous (no gap between self and other)
    const fn can_merge_with(&self, other: &Self) -> bool {
        // the access rules need to be the same
        let same_rules = self.access_rules.bits() == other.access_rules.bits();

        let overlap = self.virt.get() <= other.len.get()
            && other.virt.get() <= self.virt.get() + self.len.get();

        same_rules && overlap
    }

    /// Attempt to merge this operation with `other`.
    ///
    /// If this returns `Ok`, `other` has been merged into `self`.
    ///
    /// If this returns `Err`, `other` cannot be merged and is returned in the `Err` variant.
    fn try_merge_with(&mut self, other: Self) -> Result<(), Self> {
        if self.can_merge_with(&other) {
            let offset = self.virt.get().wrapping_sub(other.virt.get());
            let len = self
                .len
                .get()
                .checked_add(other.len.get())
                .unwrap()
                .wrapping_add(offset);

            self.virt = cmp::min(self.virt, other.virt);
            self.len = NonZero::new(len).ok_or(other)?;

            Ok(())
        } else {
            Err(other)
        }
    }
}
