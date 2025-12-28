pub mod riscv64;

use core::alloc::Layout;
use core::ops::Range;
use core::{fmt, ptr};

use crate::{MemoryAttributes, PhysicalAddress, VirtualAddress};

pub trait Arch {
    /// The type representing a single page table entry on this architecture. Usually `usize` sized.
    ///
    /// # Safety
    ///
    /// The value `0` **must** be a valid pattern for this type and **must** correspond to a _vacant_ entry.
    type PageTableEntry: PageTableEntry + fmt::Debug;

    /// The page table levels that this architecture supports.
    const LEVELS: &'static [PageTableLevel];

    /// The default base address of the [`PhysMap`][crate::PhysMap]. The loader may randomize this
    /// during ASLR but this should be the fallback address. On most architectures it is the first
    /// address of the upper-half of the address space.
    const DEFAULT_PHYSMAP_BASE: VirtualAddress;

    /// The size of the "translation granule" i.e. the smallest page size supported by this architecture.
    const GRANULE_SIZE: usize = {
        if let Some(level) = Self::LEVELS.last() {
            level.page_size()
        } else {
            unreachable!()
        }
    };

    /// A `Layout` representing a "translation granule".
    const GRANULE_LAYOUT: Layout = {
        if let Ok(layout) = Layout::from_size_align(Self::GRANULE_SIZE, Self::GRANULE_SIZE) {
            layout
        } else {
            unreachable!()
        }
    };

    /// The number of usable bits in a `VirtualAddress`. This may be used for address canonicalization.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "we check the coercion does not truncate"
    )]
    const VIRTUAL_ADDRESS_BITS: u8 = {
        let max_bits = Self::LEVELS[0].entries().ilog2();
        assert!(max_bits <= u8::MAX as u32);

        max_bits as u8 + Self::LEVELS[0].index_shift
    };

    /// Returns the physical address of the currently active page table of the calling CPU.
    fn active_table(&self) -> Option<PhysicalAddress>;

    /// Sets the currently active page table of the calling CPU to the given `address`.
    ///
    /// Note that this method **does not** establish any ordering between the page table update and
    /// implicit references to the page table, nor does it imply a page table cache flush.
    ///
    /// In the common case where page table mappings weren't modified it is not necessary to establish a
    /// barrier or flush the TLB but if you _modified mappings_ or the _address space identifier was reused_
    /// you must make sure to call [`Self::fence_all`].
    ///
    /// # Safety
    ///
    /// After this method returns, **all pointers become dangling** and as such any access through
    /// pre-existing pointers is Undefined Behaviour. This includes implicit references by the CPU
    /// such as the instruction pointer.
    ///
    /// This onerous invariant might seem impossible to uphold, if it weren't for one major exception:
    /// If a mapping is _identical_ between the two address spaces we consider it sound and allowed.
    ///
    /// This means pointers originating from _global_ mappings are safe to access after an address space
    /// switch and the same holds for identity mappings. This includes the initial bootstrapping of the
    /// kernel address space where we have to identity map the loader.
    unsafe fn set_active_table(&self, address: PhysicalAddress);

    /// Behaves like [`fence_all`][Self::fence_all] but only effect page table modifications
    /// within the given `range`.
    fn fence(&self, range: Range<VirtualAddress>);

    /// Ensures modifications to the page table are visible to the calling CPU.
    ///
    /// Instruction execution causes implicit _reads_ (and _writes_) to the page table (i.e. when
    /// the CPU translates a virtual address into a physical one for loads and stores). These implicit
    /// references are usually not ordered with respect to these loads and stores. In practice this
    /// means a CPU may pre-compute an address translation long before its associated load/store
    /// instruction or - as happens in practice - cache the translations potentially forever.
    ///
    /// This method solves this order problem by enforcing that writes to the page table are ordered _before_
    /// implicit references to the table by subsequent instructions.
    ///
    /// This will flush any local hardware caches related to address translation, a so called **"TLB flush"**.
    /// Representing it as a fence rather than a cache flush better reflects how this method interacts
    /// with instruction execution and mirrors the instructions used by out primary ISAs RISC-V and AArch64.
    fn fence_all(&self);

    /// Reads the value from `address` without moving it. This leaves the memory in `address` unchanged.
    ///
    /// # Safety
    ///
    /// This method largely inherits the safety requirements of [`ptr::read`], namely
    /// behavior is undefined if any of the following conditions are violated:
    ///
    /// - `address` must be [valid] for reads.
    /// - `address` must be properly aligned.
    /// - `address` must point to a properly initialized value of type T.
    ///
    /// Note that even if T has size 0, the pointer must be properly aligned.
    ///
    /// [valid]:
    /// [`ptr::read`]: core::ptr::read()
    unsafe fn read<T>(&self, address: VirtualAddress) -> T {
        // Safety: ensured by the caller.
        unsafe { address.as_ptr().cast::<T>().read() }
    }

    /// Overwrites the memory location pointed to by `address` with the given value without reading
    /// or dropping the old value.
    ///
    /// # Safety
    ///
    /// This method largely inherits the safety requirements of [`ptr::write`], namely
    /// behavior is undefined if any of the following conditions are violated:
    ///
    /// - `address` must be [valid] for writes.
    /// - `address` must be properly aligned.
    ///
    /// Note that even if T has size 0, the pointer must be properly aligned.
    ///
    /// [valid]:
    /// [`ptr::write`]: core::ptr::write()
    unsafe fn write<T>(&self, address: VirtualAddress, value: T) {
        // Safety: ensured by the caller.
        unsafe { address.as_mut_ptr().cast::<T>().write(value) }
    }

    /// Sets `count` bytes of memory starting at `address` to `val`.
    ///
    /// `write_bytes` behaves like C's [`memset`].
    ///
    /// [`memset`]: https://en.cppreference.com/w/c/string/byte/memset
    ///
    /// # Safety
    ///
    /// This method largely inherits the safety requirements of [`ptr::write_bytes`], namely
    /// behavior is undefined if any of the following conditions are violated:
    ///
    /// - `address` must be [valid] for writes of `count` bytes.
    /// - `address` must be properly aligned.
    ///
    /// Note that even if the effectively copied sizeis 0, the pointer must be properly aligned.
    ///
    /// [valid]:
    /// [`ptr::write_bytes`]: core::ptr::write_bytes()
    ///
    /// Additionally, note using this method one can easily introduce to undefined behavior (UB)
    /// later if the written bytes are not a valid representation of some T. **Use this to write
    /// bytes only** If you need a way to write a type to some address, use [`Self::write`].
    unsafe fn write_bytes(&self, address: VirtualAddress, value: u8, count: usize) {
        // Safety: ensured by the caller.
        unsafe { ptr::write_bytes(address.as_mut_ptr().cast::<u8>(), value, count) }
    }
}

pub trait PageTableEntry: Copy + Send {
    fn new_leaf(address: PhysicalAddress, attributes: MemoryAttributes) -> Self;
    fn new_table(address: PhysicalAddress) -> Self;
    const VACANT: Self;

    /// Returns `true` if the entry is _vacant_.
    fn is_vacant(&self) -> bool;
    /// Returns `true` if the entry is a _leaf_.
    fn is_leaf(&self) -> bool;
    /// Returns `true` if the entry is a _table_.
    fn is_table(&self) -> bool;

    /// Returns the physical address stored in this entry.
    ///
    /// This address will either be the base address of another table or the page address of a
    /// physical memory block.
    fn address(&self) -> PhysicalAddress;
    /// Returns the `MemoryAttributes` stored in this entry.
    fn attributes(&self) -> MemoryAttributes;
}

/// Represents a level in a hierarchical page table.
#[derive(Debug)]
pub struct PageTableLevel {
    /// The number of entries in this page table level
    entries: u16,
    /// Whether this page table level supports leaf entries.
    supports_leaf: bool,
    /// The number of bits we need to right-shift a `[VirtualAddress`] by to
    /// obtain its PTE index for this level. Used by [`Self::pte_index_of`].
    index_shift: u8,
}

impl PageTableLevel {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "we check the coercion does not truncate"
    )]
    pub(crate) const fn new(page_size: usize, entries: u16, supports_leaf: bool) -> PageTableLevel {
        let index_shift = page_size.ilog2();
        assert!(index_shift <= u8::MAX as u32);

        Self {
            entries,
            supports_leaf,
            index_shift: page_size.ilog2() as u8,
        }
    }

    /// Returns the number of page table entries of a table at this level.
    ///
    /// On most architectures all tables - regardless of their level - have the same
    /// number of entries. One notable exception is AArch64 where 16KiB and 64KiB
    /// page size modes have varying numbers of entries per table.
    pub const fn entries(&self) -> u16 {
        self.entries
    }

    /// Returns whether this page table level supports leaf entries.
    ///
    /// Leaf entries directly map physical memory, as opposed to pointing
    /// to the next level of the page table hierarchy.
    pub const fn supports_leaf(&self) -> bool {
        self.supports_leaf
    }

    /// The size in bytes of the memory region covered by a page table entry at this level.
    ///
    /// For example, in a 4KiB page system with 512 entries per level:
    /// - Level 0 (leaf): 4KiB (2^12)
    /// - Level 1: 2MiB (2^21)
    /// - Level 2: 1GiB (2^30)
    ///
    /// For an in-depth discussion of page sizes, block sizes, and how the naming conventions used
    /// by different architectures relate to k23's naming, see the [crate-level documentation](crate#page-size-vs-block-size).
    pub const fn page_size(&self) -> usize {
        1 << self.index_shift
    }

    /// Extracts the page table entry (PTE) for a table at this level from the given address.
    // TODO: tests
    //  - ensure this only returns in-bound indices
    pub(crate) fn pte_index_of(&self, address: VirtualAddress) -> u16 {
        let idx =
            u16::try_from(address.get() >> self.index_shift & (self.entries as usize - 1)).unwrap();
        debug_assert!(idx < self.entries);
        idx
    }

    /// Whether we can create a leaf entry at this level given the combination of base `VirtualAddress`,
    /// base `PhysicalAddress`, and remaining chunk length.
    pub(crate) fn can_map(&self, virt: VirtualAddress, phys: PhysicalAddress, len: usize) -> bool {
        let page_size = self.page_size();

        virt.is_aligned_to(page_size)
            && phys.is_aligned_to(page_size)
            && len >= page_size
            && self.supports_leaf
    }
}
