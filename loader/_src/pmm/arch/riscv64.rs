/// Number of usable bits in a virtual address
const VA_BITS: u32 = 38;
/// The smallest available page size
const PAGE_SIZE: usize = 4096;

/// The number of levels the page table has
const PAGE_TABLE_LEVELS: usize = 3; // L0, L1, L2
/// The number of page table entries in one table
const PAGE_TABLE_ENTRIES: usize = 512;

// derived constants
const PAGE_OFFSET_MASK: usize = PAGE_SIZE - 1;
/// Number of bits we need to shift an address by to reach the next page
const PAGE_SHIFT: usize = (PAGE_SIZE - 1).count_ones() as usize;
/// Number of bits we need to shift an address by to reach the next page table entry
const PAGE_ENTRY_SHIFT: usize = (PAGE_TABLE_ENTRIES - 1).count_ones() as usize;

