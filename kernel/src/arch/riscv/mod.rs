use crate::mem::VirtualAddress;
use core::range::RangeInclusive;
use static_assertions::const_assert_eq;

pub const KERNEL_ASPACE_RANGE: RangeInclusive<VirtualAddress> = RangeInclusive {
    start: VirtualAddress::new(0xffffffc000000000).unwrap(),
    end: VirtualAddress::MAX,
};
const_assert_eq!(KERNEL_ASPACE_RANGE.start.get(), CANONICAL_ADDRESS_MASK);
const_assert_eq!(
    KERNEL_ASPACE_RANGE
        .end
        .checked_sub_addr(KERNEL_ASPACE_RANGE.start)
        .unwrap(),
    !CANONICAL_ADDRESS_MASK
);

/// Virtual address where the user address space starts.
///
/// The first 2MiB are reserved for catching null pointer dereferences, but this might
/// change in the future if we decide that the null-checking performed by the WASM runtime
/// is sufficiently robust.
pub const USER_ASPACE_RANGE: RangeInclusive<VirtualAddress> = RangeInclusive {
    start: VirtualAddress::new(0x0000000000200000).unwrap(),
    end: VirtualAddress::new((1 << VIRT_ADDR_BITS) - 1).unwrap(),
};

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = (PAGE_SIZE - 1).count_ones() as usize;

pub const VIRT_ADDR_BITS: u32 = 38;
/// Canonical addresses are addresses where the tops bits (`VIRT_ADDR_BITS` to 63)
/// are all either 0 or 1.
pub const CANONICAL_ADDRESS_MASK: usize = !((1 << (VIRT_ADDR_BITS)) - 1);
const_assert_eq!(CANONICAL_ADDRESS_MASK, 0xffffffc000000000);
