use std::cell::{Ref, RefCell, RefMut};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::ops::Range;
use std::sync::Arc;
use std::{cmp, fmt};

use arrayvec::ArrayVec;
use cpu_local::collection::CpuLocal;

use crate::arch::{Arch, PageTableEntry, PageTableLevel};
use crate::bootstrap::BootstrapAllocator;
use crate::flush::Flush;
use crate::test_utils::arch::EmulateArch;
use crate::test_utils::memory::Memory;
use crate::utils::page_table_entries_for;
use crate::{
    AllocError, HardwareAddressSpace, MemoryAttributes, PhysMap, PhysicalAddress, VirtualAddress,
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
    pub fn memory_regions<const MAX: usize>(&self) -> ArrayVec<Range<PhysicalAddress>, MAX> {
        self.0.memory.regions().collect()
    }

    pub unsafe fn read<T>(&self, asid: u16, addr: VirtualAddress) -> T {
        assert!(addr.is_aligned_to(size_of::<T>()));

        if let Some((phys, attrs, _level)) = self.cpu().translate(asid, addr) {
            assert!(attrs.allows_read());

            unsafe { self.read_phys(phys) }
        } else {
            core::panic!("read: {addr} size {:#x} not present", size_of::<T>());
        }
    }

    pub unsafe fn write<T>(&self, asid: u16, addr: VirtualAddress, value: T) {
        assert!(addr.is_aligned_to(size_of::<T>()));

        if let Some((phys, attrs, _level)) = self.cpu().translate(asid, addr) {
            assert!(attrs.allows_read());

            unsafe { self.write_phys(phys, value) }
        } else {
            core::panic!("write: {addr} size {:#x} not present", size_of::<T>());
        }
    }

    pub fn write_bytes(&self, asid: u16, address: VirtualAddress, value: u8, count: usize) {
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

    pub unsafe fn read_phys<T>(&self, address: PhysicalAddress) -> T {
        unsafe { self.0.memory.read(address) }
    }

    pub unsafe fn write_phys<T>(&self, address: PhysicalAddress, value: T) {
        unsafe { self.0.memory.write(address, value) }
    }

    pub fn write_bytes_phys(&self, address: PhysicalAddress, value: u8, count: usize) {
        self.0.memory.write_bytes(address, value, count)
    }

    pub fn active_table(&self) -> Option<PhysicalAddress> {
        self.cpu().active_page_table()
    }

    pub unsafe fn set_active_table(&self, address: PhysicalAddress) {
        self.cpu_mut().set_active_page_table(address);
    }

    pub fn invalidate(&self, asid: u16, address_range: Range<VirtualAddress>) {
        let mut cpu = self.cpu_mut();

        cpu.invalidate(asid, address_range, &self.0.memory);
    }

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
            VirtualAddress::MIN..VirtualAddress::MAX.align_down(A::GRANULE_SIZE),
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

pub struct MachineBuilder<A: Arch, R: lock_api::RawMutex, Mem> {
    memory: Option<Memory>,
    physmap_base: VirtualAddress,
    _has: PhantomData<Mem>,
    _m: PhantomData<(A, R)>,
}

pub struct BootstrapResult<A: Arch, R: lock_api::RawMutex> {
    pub machine: Machine<A>,
    pub address_space: HardwareAddressSpace<EmulateArch<A>>,
    pub frame_allocator: BootstrapAllocator<R>,
}

impl<A: Arch, R: lock_api::RawMutex> MachineBuilder<A, R, MissingMemory> {
    pub fn new() -> Self {
        Self {
            memory: None,
            physmap_base: A::DEFAULT_PHYSMAP_BASE,
            _has: PhantomData,
            _m: PhantomData,
        }
    }
}

impl<A: Arch, R: lock_api::RawMutex> MachineBuilder<A, R, MissingMemory> {
    pub fn with_memory_regions(
        self,
        region_sizes: impl IntoIterator<Item = usize>,
    ) -> MachineBuilder<A, R, HasMemory> {
        let memory = Memory::new::<A>(region_sizes);

        assert!(
            memory.regions().next().is_some(),
            "you must specify at least one memory region"
        );

        MachineBuilder {
            memory: Some(memory),
            physmap_base: self.physmap_base,
            _has: PhantomData,
            _m: PhantomData,
        }
    }
}

impl<A: Arch, R: lock_api::RawMutex> MachineBuilder<A, R, HasMemory> {
    pub fn finish(self) -> (Machine<A>, PhysMap) {
        let memory = self.memory.unwrap();

        let physmap = PhysMap::new(self.physmap_base, memory.regions());

        let inner = MachineInner {
            memory,
            cpus: CpuLocal::with_capacity(std::thread::available_parallelism().unwrap().get()),
        };

        (Machine(Arc::new(inner)), physmap)
    }

    pub fn finish_and_bootstrap(self) -> Result<BootstrapResult<A, R>, AllocError> {
        let (machine, physmap) = self.finish();

        let arch = EmulateArch::new(machine.clone());

        let frame_allocator = BootstrapAllocator::new::<A>(arch.machine().memory_regions());

        let mut flush = Flush::new();
        let mut aspace =
            HardwareAddressSpace::new_bootstrap(arch, physmap, &frame_allocator, &mut flush)?;

        aspace.map_physical_memory(&frame_allocator, &mut flush)?;

        // Safety: we just created the address space, so don't have any pointers into it. In hosted tests
        // the programs memory and CPU registers are outside the address space anyway.
        let address_space = unsafe { aspace.finish_bootstrap_and_activate() };

        flush.flush(address_space.arch());

        Ok(BootstrapResult {
            machine,
            address_space,
            frame_allocator,
        })
    }
}
