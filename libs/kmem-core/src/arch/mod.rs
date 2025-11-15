// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#[cfg(feature = "emulate")]
pub mod emulate;
pub mod riscv64;

use core::ops::Range;
use core::ptr;

use crate::{MemoryAttributes, MemoryMode, PhysicalAddress, VirtualAddress};

/// Architecture-specific memory subsystem primitives.
pub trait Arch {
    /// The type representing a single page table entry on this architecture. Usually `usize` sized.
    ///
    /// # Safety
    ///
    /// The value `0` **must** be a valid pattern for this type and **must** correspond to a _vacant_ entry.
    type PageTableEntry: PageTableEntry;

    /// The memory mode used by the calling CPU to translate virtual address to physical addresses.
    fn memory_mode(&self) -> &'static MemoryMode;

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

    fn phys_to_virt(&self, address: PhysicalAddress) -> VirtualAddress {
        address.to_virt(self.memory_mode().physmap_base())
    }
}

/// The type representing a single page table entry on this architecture. Usually `usize` sized.
pub trait PageTableEntry: Copy + Send {
    /// Returns a new _leaf_ entry, i.e. one that directly maps a block of physical memory.
    fn new_leaf(address: PhysicalAddress, attributes: MemoryAttributes) -> Self;
    /// Returns a new _table_ entry, i.e. one that refers to another table in the page table hierarchy.
    fn new_table(address: PhysicalAddress) -> Self;
    /// Returns a new _vacant_ entry, i.e. one that is invalid and will cause a page fault when
    /// its mapping is accessed.
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
