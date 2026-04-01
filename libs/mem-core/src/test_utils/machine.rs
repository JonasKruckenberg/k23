use std::alloc::Layout;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::range::Range;
use std::sync::Arc;
use std::{cmp, fmt};

use karrayvec::ArrayVec;
use kcpu_local::collection::CpuLocal;

use crate::arch::{Arch, PageTableEntry, PageTableLevel};
use crate::frame_allocator::BumpAllocator;
use crate::test_utils::arch::EmulateArch;
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
    pub fn bootstrap_address_space(
        &self,
        physmap_start: VirtualAddress,
    ) -> (
        HardwareAddressSpace<EmulateArch<A>>,
        BumpAllocator<parking_lot::RawMutex>,
        PhysMap,
    ) {
        let arch = EmulateArch::new(self.clone());

        let memory_regions: ArrayVec<_, _> = arch.machine().memory_regions().collect();

        let active_physmap = PhysMap::ABSENT;
        let chosen_physmap = PhysMap::new(physmap_start, memory_regions.clone());

        let frame_allocator = BumpAllocator::new::<A>(memory_regions.clone());

        let mut address_space = HardwareAddressSpace::new(arch, &active_physmap, frame_allocator.by_ref())
            .expect("Machine does not have enough physical memory for root page table. Consider increasing configured physical memory sizes.");

        address_space.map_physical_memory(memory_regions.into_iter(), &active_physmap, &chosen_physmap, frame_allocator.by_ref())
            .expect("Machine does not have enough physical memory for physmap. Consider increasing configured physical memory sizes.");

        // Safety: we just created the address space, so don't have any pointers into it. In hosted tests
        // the programs memory and CPU registers are outside the address space anyway.
        let address_space = unsafe { address_space.finish_bootstrap_and_activate() };

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
            assert_eq!(
                address.align_down(level.page_size()),
                address.add(size_of::<T>()).align_down(level.page_size()),
                "reads crossing page boundaries are not supported. {address} + {}",
                size_of::<T>()
            );

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
            assert_eq!(
                address.align_down(level.page_size()),
                address.add(size_of::<T>()).align_down(level.page_size()),
                "typed writes crossing page boundaries are not supported. {address} + {}",
                size_of::<T>()
            );

            unsafe { self.write_phys(phys, value) }
        } else {
            core::panic!("write: {address} size {:#x} not present", size_of::<T>());
        }
    }

    /// Reads `count` bytes of memory starting at `address`. This leaves the memory in `address` unchanged.
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
    pub unsafe fn read_bytes(&self, asid: u16, address: VirtualAddress, count: usize) -> &[u8] {
        if let Some((phys, attrs, level)) = self.cpu().translate(asid, address) {
            assert!(attrs.allows_read());
            assert_eq!(
                address.align_down(level.page_size()),
                address.add(count).align_down(level.page_size()),
                "reads crossing page boundaries are not supported. {address} + {}",
                count
            );

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
        self.0.memory.write_bytes(address, value, count)
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

    pub fn invalidate(&mut self, asid: u16, range: Range<VirtualAddress>, memory: &Memory) {
        self.map
            .retain(|(key_asid, key_range), _| !(*key_asid == asid && range.contains(key_range)));

        self.reload_map(asid, range, 0, self.page_table.unwrap(), memory);
    }

    pub fn invalidate_all(&mut self, asid: u16, memory: &Memory) {
        self.map.clear();

        self.reload_map(
            asid,
            Range {
                start: VirtualAddress::MIN,
                end: VirtualAddress::MAX.align_down(A::GRANULE_SIZE),
            },
            0,
            self.page_table.unwrap(),
            memory,
        );
    }

    fn reload_map(
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
            let entry = unsafe {
                memory.read::<A::PageTableEntry>(
                    table.add(pte_index as usize * size_of::<A::PageTableEntry>()),
                )
            };

            if entry.is_table() {
                self.reload_map(asid, range, depth + 1, entry.address(), memory);
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
    pub fn finish(self) -> Machine<A> {
        let memory = self.memory.unwrap();

        let inner = MachineInner {
            memory,
            cpus: CpuLocal::with_capacity(std::thread::available_parallelism().unwrap().get()),
        };

        Machine(Arc::new(inner))
    }
}
