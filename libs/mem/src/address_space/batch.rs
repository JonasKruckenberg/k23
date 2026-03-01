use core::cmp;
use core::range::Range;

use kmem_core::{
    AddressRangeExt, AllocError, Arch, Flush, FrameAllocator, HardwareAddressSpace,
    MemoryAttributes, PhysMap, PhysicalAddress, VirtualAddress,
};
use smallvec::SmallVec;

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
    Map(Map),
    Unmap(Unmap),
    SetAttributes(SetAttributes),
}

struct Map {
    virt: Range<VirtualAddress>,
    phys: Range<PhysicalAddress>,
    attributes: MemoryAttributes,
}

struct Unmap {
    range: Range<VirtualAddress>,
}

struct SetAttributes {
    range: Range<VirtualAddress>,
    attributes: MemoryAttributes,
}

impl Batch {
    pub fn new() -> Self {
        Self {
            ops: SmallVec::new(),
        }
    }

    #[inline]
    pub fn map(
        &mut self,
        virt: Range<VirtualAddress>,
        phys: PhysicalAddress,
        attributes: MemoryAttributes,
    ) {
        let mut new_op = Map {
            phys: Range::from_start_len(phys, virt.len()),
            virt,
            attributes,
        };

        let ops = self.ops.iter_mut().filter_map(|op| match op {
            BatchOperation::Map(op) => Some(op),
            _ => None,
        });

        for op in ops {
            match op.try_merge_with(new_op) {
                Ok(()) => return,
                Err(new_op_) => new_op = new_op_,
            }
        }

        self.ops.push(BatchOperation::Map(new_op));
    }

    #[inline]
    pub fn unmap(&mut self, range: Range<VirtualAddress>) {
        let mut new_op = Unmap { range };

        let ops = self.ops.iter_mut().filter_map(|op| match op {
            BatchOperation::Unmap(op) => Some(op),
            _ => None,
        });

        for op in ops {
            match op.try_merge_with(new_op) {
                Ok(()) => return,
                Err(new_op_) => new_op = new_op_,
            }
        }

        self.ops.push(BatchOperation::Unmap(new_op));
    }

    #[inline]
    pub fn set_memory_attributes(
        &mut self,
        range: Range<VirtualAddress>,
        attributes: MemoryAttributes,
    ) {
        let mut new_op = SetAttributes { range, attributes };

        let ops = self.ops.iter_mut().filter_map(|op| match op {
            BatchOperation::SetAttributes(op) => Some(op),
            _ => None,
        });

        for op in ops {
            match op.try_merge_with(new_op) {
                Ok(()) => return,
                Err(new_op_) => new_op = new_op_,
            }
        }

        self.ops.push(BatchOperation::SetAttributes(new_op));
    }

    /// Flushes the `Batch` ensuring all changes are materialized into the raw address space.
    pub fn flush_changes(
        &mut self,
        hardware_address_space: &mut HardwareAddressSpace<impl Arch>,
        physmap: &PhysMap,
        frame_allocator: impl FrameAllocator,
    ) -> Result<(), AllocError> {
        let mut flush = Flush::new();

        for op in self.ops.drain(..) {
            match op {
                BatchOperation::Map(op) => op.flush_changes(
                    hardware_address_space,
                    physmap,
                    frame_allocator.by_ref(),
                    &mut flush,
                )?,
                BatchOperation::Unmap(op) => op.flush_changes(
                    hardware_address_space,
                    physmap,
                    frame_allocator.by_ref(),
                    &mut flush,
                ),
                BatchOperation::SetAttributes(op) => {
                    op.flush_changes(hardware_address_space, physmap, &mut flush)
                }
            };
        }

        flush.flush(hardware_address_space.arch());

        Ok(())
    }
}

impl Map {
    /// Returns true if this operation can be merged with `other`.
    ///
    /// map operations can be merged if:
    /// - their [`MemoryAttributes`] are the same
    /// - their virtual address ranges are contiguous (no gap between self and other)
    fn can_merge_with(&self, other: &Self) -> bool {
        // the access rules need to be the same
        let same_rules = self.attributes.bits() == other.attributes.bits();

        // and regions must overlap or adjacent
        let overlaps_virt = self.virt.start <= other.virt.end && other.virt.start <= self.virt.end;

        let overlaps_phys = self.phys.start <= other.phys.end && other.phys.start <= self.phys.end;

        same_rules && overlaps_virt && overlaps_phys
    }

    fn try_merge_with(&mut self, other: Self) -> Result<(), Self> {
        if self.can_merge_with(&other) {
            self.virt = Range {
                start: cmp::min(self.virt.start, other.virt.start),
                end: cmp::max(self.virt.end, other.virt.end),
            };

            self.phys = Range {
                start: cmp::min(self.phys.start, other.phys.start),
                end: cmp::max(self.phys.end, other.phys.end),
            };

            Ok(())
        } else {
            Err(other)
        }
    }

    fn flush_changes(
        self,
        hardware_address_space: &mut HardwareAddressSpace<impl Arch>,
        physmap: &PhysMap,
        frame_allocator: impl FrameAllocator,
        flush: &mut Flush,
    ) -> Result<(), AllocError> {
        // Safety: promised by the caller when constructing the operation
        unsafe {
            hardware_address_space.map_contiguous(
                self.virt,
                self.phys.start,
                self.attributes,
                frame_allocator,
                physmap,
                flush,
            )
        }
    }
}

impl Unmap {
    /// Returns true if this operation can be merged with `other`.
    ///
    /// Unmap operations can be merged if:
    /// - their virtual address ranges are contiguous (no gap between self and other)
    fn can_merge_with(&self, other: &Self) -> bool {
        self.range.start <= other.range.end && other.range.start <= self.range.end
    }

    fn try_merge_with(&mut self, other: Self) -> Result<(), Self> {
        if self.can_merge_with(&other) {
            self.range = Range {
                start: cmp::min(self.range.start, other.range.start),
                end: cmp::max(self.range.end, other.range.end),
            };

            Ok(())
        } else {
            Err(other)
        }
    }

    fn flush_changes(
        self,
        hardware_address_space: &mut HardwareAddressSpace<impl Arch>,
        physmap: &PhysMap,
        frame_allocator: impl FrameAllocator,
        flush: &mut Flush,
    ) {
        // Safety: promised by the caller when constructing the operation
        unsafe {
            hardware_address_space.unmap(self.range, frame_allocator, physmap, flush);
        }
    }
}

impl SetAttributes {
    /// Returns true if this operation can be merged with `other`.
    ///
    /// set attribute operations can be merged if:
    /// - their [`MemoryAttributes`] are the same
    /// - their virtual address ranges are contiguous (no gap between self and other)
    fn can_merge_with(&self, other: &Self) -> bool {
        // the access rules need to be the same
        let same_rules = self.attributes.bits() == other.attributes.bits();

        // and regions must overlap or adjacent
        let overlaps = self.range.start <= other.range.end && other.range.start <= self.range.end;

        same_rules && overlaps
    }

    fn try_merge_with(&mut self, other: Self) -> Result<(), Self> {
        if self.can_merge_with(&other) {
            self.range = Range {
                start: cmp::min(self.range.start, other.range.start),
                end: cmp::max(self.range.end, other.range.end),
            };

            Ok(())
        } else {
            Err(other)
        }
    }

    fn flush_changes(
        self,
        hardware_address_space: &mut HardwareAddressSpace<impl Arch>,
        physmap: &PhysMap,
        flush: &mut Flush,
    ) {
        // Safety: promised by the caller when constructing the operation
        unsafe {
            hardware_address_space.set_attributes(self.range, self.attributes, physmap, flush);
        }
    }
}
