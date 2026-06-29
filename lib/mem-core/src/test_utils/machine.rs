// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use std::alloc::Layout;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::range::Range;
use std::sync::Arc;
use std::{cmp, fmt};

use cpu_local::collection::CpuLocal;

use crate::arch::{Arch, PageTableEntry, PageTableLevel};
use crate::test_utils::arch::EmulateArch;
use crate::test_utils::frame_allocator::TestFrameAllocator;
use crate::test_utils::memory::Memory;
use crate::utils::page_table_entries_for;
use crate::{
    FrameAllocator, HardwareAddressSpace, MemoryAttributes, PhysMap, PhysicalAddress,
    VirtualAddress,
};

/// A "virtual machine" that emulates a given architecture. It is intended to be used in tests
/// and supports modeling the following properties:
///
/// - multiple, discontiguous physical memory regions
/// - per-cpu virtual->physical address translation buffers
/// - address translation buffer invalidation
pub struct Machine<A: Arch>(Arc<MachineInner<A>>);

struct MachineInner<A: Arch> {
    memory: Memory,
    cpus: CpuLocal<RefCell<Cpu<A>>>,
}

impl<A: Arch> Clone for Machine<A> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<A: Arch> fmt::Debug for Machine<A>
where
    A::PageTableEntry: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Machine")
            .field("memory", &self.0.memory)
            .field("current_cpu", &self.0.cpus)
            .finish()
    }
}

impl<A: Arch> Machine<A> {
    /// Bootstrap an address space for this machine. Will set up initial page table and
    /// frame allocator.
    ///
    /// # Panics
    ///
    /// Panics if the machine lacks enough physical memory for the root page table or the physmap.
    pub fn bootstrap_address_space(
        &self,
        physmap_start: VirtualAddress,
    ) -> (
        HardwareAddressSpace<EmulateArch<A>>,
        TestFrameAllocator,
        PhysMap,
    ) {
        let arch = EmulateArch::new(self.clone());

        let memory_regions: Vec<_> = arch.machine().memory_regions().collect();

        let active_physmap = PhysMap::new_identity(memory_regions.clone());
        let chosen_physmap = PhysMap::new(physmap_start, memory_regions.clone());

        let frame_allocator = TestFrameAllocator::new::<A>(memory_regions.clone());

        let mut address_space = HardwareAddressSpace::new(arch, &active_physmap, frame_allocator.by_ref())
            .expect("Machine does not have enough physical memory for root page table. Consider increasing configured physical memory sizes.");

        address_space.map_physical_memory(memory_regions.into_iter(), &active_physmap, &chosen_physmap, frame_allocator.by_ref())
            .expect("Machine does not have enough physical memory for physmap. Consider increasing configured physical memory sizes.");

        // Safety: we just created the address space, so don't have any pointers into it. In hosted tests
        // the programs memory and CPU registers are outside the address space anyway.
        unsafe { address_space.activate() };

        (address_space, frame_allocator, chosen_physmap)
    }

    /// Returns an iterator over the physical memory regions in this machine
    pub fn memory_regions(&self) -> impl Iterator<Item = Range<PhysicalAddress>> {
        self.0.memory.regions()
    }

    /// Reads the value from `address` without moving it. This leaves the memory in `address` unchanged.
    ///
    /// This method **does not** support reads crossing page boundaries.
    ///
    /// # Panics
    ///
    /// Panics if the address range is not mapped as readable.
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
    pub unsafe fn read<T>(&self, asid: u16, address: VirtualAddress) -> T {
        assert!(address.is_aligned_to(size_of::<T>()));

        if let Some((phys, attrs, level)) = self.cpu().translate(asid, address) {
            assert!(attrs.allows_read());
            // NB: a read of N bytes touches `[address, address + N)`.
            // the last byte is at `address + N - 1`.
            assert_eq!(
                address.align_down(level.page_size()),
                address
                    .add(size_of::<T>().saturating_sub(1))
                    .align_down(level.page_size()),
                "typed reads crossing page boundaries are not supported. {address} + {}",
                size_of::<T>()
            );

            // Safety: validity/alignment ensured by caller
            unsafe { self.read_phys(phys) }
        } else {
            core::panic!("read: {address} size {:#x} not present", size_of::<T>());
        }
    }

    /// Overwrites the memory location pointed to by `address` with the given value without reading
    /// or dropping the old value.
    ///
    /// This method **does not** support writes crossing page boundaries.
    ///
    /// # Panics
    ///
    /// Panics if the address range is not mapped as writable.
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
    pub unsafe fn write<T>(&self, asid: u16, address: VirtualAddress, value: T) {
        assert!(address.is_aligned_to(size_of::<T>()));

        if let Some((phys, attrs, level)) = self.cpu().translate(asid, address) {
            assert!(attrs.allows_write());
            // NB: a write of N bytes touches `[address, address + N)`.
            // the last byte is at `address + N - 1`.
            assert_eq!(
                address.align_down(level.page_size()),
                address
                    .add(size_of::<T>().saturating_sub(1))
                    .align_down(level.page_size()),
                "typed writes crossing page boundaries are not supported. {address} + {}",
                size_of::<T>()
            );

            // Safety: validity/alignment ensured by caller
            unsafe { self.write_phys(phys, value) }
        } else {
            core::panic!("write: {address} size {:#x} not present", size_of::<T>());
        }
    }

    /// Reads `count` bytes of memory starting at `address`. This leaves the memory in `address` unchanged.
    ///
    /// This method **does not** support reads crossing page boundaries.
    ///
    /// # Panics
    ///
    /// Panics if the address range is not mapped as readable.
    ///
    /// # Safety
    ///
    /// This method largely inherits the safety requirements of [`slice::from_raw_parts`], namely
    /// behavior is undefined if any of the following conditions are violated:
    ///
    /// - `address` must be non-null and [valid] for reads of `count` bytes.
    /// - `address` must be properly aligned.
    /// - The memory referenced by the returned slice must not be mutated for the duration its lifetime.
    pub unsafe fn read_bytes(&self, asid: u16, address: VirtualAddress, count: usize) -> &[u8] {
        if let Some((phys, attrs, level)) = self.cpu().translate(asid, address) {
            assert!(attrs.allows_read());
            // NB: a read of N bytes touches `[address, address + N)`.
            // the last byte is at `address + N - 1`.
            assert_eq!(
                address.align_down(level.page_size()),
                address
                    .add(count.saturating_sub(1))
                    .align_down(level.page_size()),
                "reads crossing page boundaries are not supported. {address} + {}",
                count
            );

            // Safety: validity ensured by caller
            self.read_bytes_phys(phys, count)
        } else {
            panic!("write: {address} size {count:#x} not present");
        }
    }

    /// Sets `count` bytes of memory starting at `address` to `val`.
    ///
    /// `write_bytes` behaves like C's [`memset`].
    ///
    /// [`memset`]: https://en.cppreference.com/w/c/string/byte/memset
    ///
    /// Contrary to [`Self::read`], [`Self::write`], and [`Self::write_bytes`] this **does**
    /// support writes crossing page boundaries.
    ///
    /// # Panics
    ///
    /// Panics if the address range is not mapped as writable.
    ///
    /// # Safety
    ///
    /// This method largely inherits the safety requirements of [`ptr::write_bytes`], namely
    /// behavior is undefined if any of the following conditions are violated:
    ///
    /// - `address` must be non-null and [valid] for writes of `count` bytes.
    /// - `address` must be properly aligned.
    ///
    /// Note that even if the effectively copied size is 0, the pointer must be properly aligned.
    ///
    /// [valid]:
    /// [`ptr::write_bytes`]: core::ptr::write_bytes()
    ///
    /// Additionally, note using this method one can easily introduce to undefined behavior (UB)
    /// later if the written bytes are not a valid representation of some T. **Use this to write
    /// bytes only** If you need a way to write a type to some address, use [`Self::write`].
    pub unsafe fn write_bytes(&self, asid: u16, address: VirtualAddress, value: u8, count: usize) {
        let mut bytes_remaining = count;
        let mut address = address;

        while bytes_remaining > 0 {
            if let Some((phys, attrs, level)) = self.cpu().translate(asid, address) {
                assert!(attrs.allows_write());

                let write_size = cmp::min(bytes_remaining, level.page_size());

                self.write_bytes_phys(phys, value, write_size);

                address = address.add(write_size);
                bytes_remaining -= write_size;
            } else {
                panic!("write: {address} size {count:#x} not present");
            }
        }
    }

    /// Reads the value from physical address `address` bypassing address translation and attribute
    /// checks. Reads the value without moving it leaving the memory in `address` unchanged.
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
    pub unsafe fn read_phys<T>(&self, address: PhysicalAddress) -> T {
        // Safety: validity/alignment ensured by caller
        unsafe { self.0.memory.read(address) }
    }

    /// Overwrites the memory location pointed to by physical address `address` bypassing address
    /// translation and attribute checks. Overwrites the location with the given value without reading
    /// or dropping the old value.
    ///
    /// This method **does not** support writes crossing page boundaries.
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
    pub unsafe fn write_phys<T>(&self, address: PhysicalAddress, value: T) {
        // Safety: validity/alignment ensured by caller
        unsafe { self.0.memory.write(address, value) }
    }

    /// Reads `count` bytes of memory starting at physical address `address` bypassing address
    /// translation and attribute checks. This leaves the memory in `address` unchanged.
    ///
    /// This method **does not** support reads crossing page boundaries.
    ///
    /// # Safety
    ///
    /// This method largely inherits the safety requirements of [`slice::from_raw_parts`], namely
    /// behavior is undefined if any of the following conditions are violated:
    ///
    /// - `address` must be non-null and [valid] for reads of `count` bytes.
    /// - `address` must be properly aligned.
    /// - The memory referenced by the returned slice must not be mutated for the duration its lifetime.
    pub fn read_bytes_phys(&self, address: PhysicalAddress, count: usize) -> &[u8] {
        self.0.memory.read_bytes(address, count)
    }

    /// Sets `count` bytes of memory starting at physical address `address` to `val`. This
    /// bypassing address translation and attribute checks.
    ///
    /// `write_bytes` behaves like C's [`memset`].
    ///
    /// [`memset`]: https://en.cppreference.com/w/c/string/byte/memset
    ///
    /// Contrary to [`Self::read`], [`Self::write`], and [`Self::write_bytes`] this **does**
    /// support writes crossing page boundaries.
    ///
    /// # Safety
    ///
    /// This method largely inherits the safety requirements of [`ptr::write_bytes`], namely
    /// behavior is undefined if any of the following conditions are violated:
    ///
    /// - `address` must be non-null and [valid] for writes of `count` bytes.
    /// - `address` must be properly aligned.
    ///
    /// Note that even if the effectively copied size is 0, the pointer must be properly aligned.
    ///
    /// [valid]:
    /// [`ptr::write_bytes`]: core::ptr::write_bytes()
    ///
    /// Additionally, note using this method one can easily introduce to undefined behavior (UB)
    /// later if the written bytes are not a valid representation of some T. **Use this to write
    /// bytes only** If you need a way to write a type to some address, use [`Self::write`].
    pub fn write_bytes_phys(&self, address: PhysicalAddress, value: u8, count: usize) {
        self.0.memory.write_bytes(address, value, count);
    }

    /// Return the active page table on the calling (emulated) CPU (thread).
    pub fn active_table(&self) -> Option<PhysicalAddress> {
        self.cpu().active_page_table()
    }

    /// Sets the active page table on the calling (emulated) CPU (thread).
    pub unsafe fn set_active_table(&self, address: PhysicalAddress) {
        self.cpu_mut().set_active_page_table(address);
    }

    /// Invalidates existing virtual address translation entries for address space `asid` in the
    /// give `address_range`.
    pub fn invalidate(&self, asid: u16, address_range: Range<VirtualAddress>) {
        let mut cpu = self.cpu_mut();

        cpu.invalidate(asid, address_range, &self.0.memory);
    }

    /// Invalidates all existing virtual address translation entries for address space `asid`.
    pub fn invalidate_all(&self, asid: u16) {
        let mut cpu = self.cpu_mut();

        cpu.invalidate_all(asid, &self.0.memory);
    }

    fn cpu(&self) -> Ref<'_, Cpu<A>> {
        self.0.cpus.get_or(|| RefCell::new(Cpu::new())).borrow()
    }

    fn cpu_mut(&self) -> RefMut<'_, Cpu<A>> {
        self.0.cpus.get_or(|| RefCell::new(Cpu::new())).borrow_mut()
    }
}

pub struct Cpu<A: Arch> {
    map: BTreeMap<
        (u16, VirtualAddress),
        (VirtualAddress, A::PageTableEntry, &'static PageTableLevel),
    >,
    page_table: Option<PhysicalAddress>,
}

impl<A: Arch> fmt::Debug for Cpu<A>
where
    A::PageTableEntry: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cpu")
            .field("cache", &self.map)
            .field("page_table", &self.page_table)
            .finish()
    }
}

impl<A: Arch> Cpu<A> {
    pub fn new() -> Self {
        Cpu {
            map: BTreeMap::new(),
            page_table: None,
        }
    }

    pub fn translate(
        &self,
        asid: u16,
        address: VirtualAddress,
    ) -> Option<(PhysicalAddress, MemoryAttributes, &'static PageTableLevel)> {
        let (_end, (start, entry, level)) = self.map.range((asid, address)..).next()?;
        let offset = address.get().checked_sub(start.get())?;

        Some((entry.address().add(offset), entry.attributes(), level))
    }

    pub fn active_page_table(&self) -> Option<PhysicalAddress> {
        self.page_table
    }

    pub fn set_active_page_table(&mut self, address: PhysicalAddress) {
        self.page_table = Some(address);
    }

    /// Invalidate page table mappings for the given virtual address `range` and `asid`.
    ///
    /// # Panics
    ///
    /// Panics if no page table is active on the calling CPU.
    pub fn invalidate(&mut self, asid: u16, range: Range<VirtualAddress>, memory: &Memory) {
        self.map
            .retain(|(key_asid, key_range), _| !(*key_asid == asid && range.contains(key_range)));

        // Safety: `self.page_table` is set by the hardware address space
        unsafe {
            self.reload_map(asid, range, 0, self.page_table.unwrap(), memory);
        }
    }

    /// Invalidate all page table mappings for the given `asid`.
    ///
    /// # Panics
    ///
    /// Panics if no page table is active on the calling CPU.
    pub fn invalidate_all(&mut self, asid: u16, memory: &Memory) {
        self.map.clear();

        // Safety: `self.page_table` is set by the hardware address space
        unsafe {
            self.reload_map(
                asid,
                Range::from(VirtualAddress::MIN..VirtualAddress::MAX.align_down(A::GRANULE_SIZE)),
                0,
                self.page_table.unwrap(),
                memory,
            );
        }
    }

    /// Reload the translation map for the calling CPU.
    ///
    /// # Safety
    ///
    /// The caller must ensure `table` points at a valid, initialized page table.
    unsafe fn reload_map(
        &mut self,
        asid: u16,
        range: Range<VirtualAddress>,
        depth: u8,
        table: PhysicalAddress,
        memory: &Memory,
    ) {
        let level = &A::LEVELS[depth as usize];

        log::trace!("reloading map chunk {range:?}");
        let entries = page_table_entries_for::<A>(range, level);

        for (pte_index, range) in entries {
            // Safety: ensured by caller
            let entry = unsafe {
                memory.read::<A::PageTableEntry>(
                    table.add(pte_index as usize * size_of::<A::PageTableEntry>()),
                )
            };

            if entry.is_table() {
                // Safety: `entry.address()` is the frame this table descriptor points at; its
                // validity is guaranteed by this function's own contract.
                unsafe {
                    self.reload_map(asid, range, depth + 1, entry.address(), memory);
                }
            } else if entry.is_leaf() {
                log::trace!("inserting map entry for {range:?}");
                self.map
                    .insert((asid, range.end), (range.start, entry, level));
            }
        }
    }
}

pub struct MissingMemory;

pub struct HasMemory;

pub struct MachineBuilder<A: Arch, Mem> {
    // under_construction: HardwareAddressSpace<A, Bootstrapping>,
    memory: Option<Memory>,
    _has: PhantomData<Mem>,
    _m: PhantomData<A>,
}

impl<A: Arch> MachineBuilder<A, MissingMemory> {
    pub fn new() -> Self {
        Self {
            memory: None,
            _has: PhantomData,
            _m: PhantomData,
        }
    }
}

impl<A: Arch> MachineBuilder<A, MissingMemory> {
    /// Sets the size and alignments(s) of the machines physical memory regions. The exact
    /// addresses will be chosen at random and can be retrieved via [`Machine::memory_regions`].
    ///
    /// # Panics
    ///
    /// Panics if the memory regions iterator is empty.
    pub fn with_memory_regions(
        self,
        region_sizes: impl IntoIterator<Item = Layout>,
    ) -> MachineBuilder<A, HasMemory> {
        let memory = Memory::new::<A>(region_sizes);

        assert!(
            memory.regions().next().is_some(),
            "you must specify at least one memory region"
        );

        MachineBuilder {
            memory: Some(memory),
            _has: PhantomData,
            _m: PhantomData,
        }
    }
}

impl<A: Arch> MachineBuilder<A, HasMemory> {
    /// Finish constructing and return the machine.
    #[expect(clippy::missing_panics_doc, reason = "internal assertion")]
    pub fn finish(self) -> Machine<A> {
        let memory = self.memory.unwrap();

        let inner = MachineInner {
            memory,
            cpus: CpuLocal::with_capacity(std::thread::available_parallelism().unwrap().get()),
        };

        Machine(Arc::new(inner))
    }
}
