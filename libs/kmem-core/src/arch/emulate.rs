// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::cell::RefCell;
use core::marker::PhantomData;
use core::ops::Range;
use core::{cmp, fmt, ptr};
use std::boxed::Box;
use std::collections::BTreeMap;

use arrayvec::ArrayVec;
use cpu_local::collection::CpuLocal;
use lock_api::Mutex;

use crate::arch::PageTableEntry;
use crate::utils::page_table_entries_for;
use crate::{Arch, MemoryAttributes, MemoryMode, PageTableLevel, PhysicalAddress, VirtualAddress};

pub struct EmulateArch<A: Arch, R: lock_api::RawMutex> {
    machine: Machine<A, R>,
    asid: u16,
}

impl<A: Arch, R: lock_api::RawMutex> fmt::Debug for EmulateArch<A, R>
where
    A::PageTableEntry: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmulateArch")
            .field("machine", &self.machine)
            .finish()
    }
}

impl<A: Arch, R: lock_api::RawMutex> EmulateArch<A, R> {
    pub fn new(machine: Machine<A, R>) -> Self {
        Self { machine, asid: 0 }
    }

    pub fn with_asid(machine: Machine<A, R>, asid: u16) -> Self {
        Self { machine, asid }
    }

    pub fn machine(&self) -> &Machine<A, R> {
        &self.machine
    }
}

impl<A: Arch, R: lock_api::RawMutex> Arch for EmulateArch<A, R> {
    type PageTableEntry = A::PageTableEntry;

    fn memory_mode(&self) -> &'static MemoryMode {
        self.machine.memory_mode
    }

    fn active_table(&self) -> Option<PhysicalAddress> {
        self.machine.active_table()
    }

    unsafe fn set_active_table(&self, address: PhysicalAddress) {
        unsafe {
            self.machine.set_active_table(address);
        }
    }

    fn fence(&self, address_range: Range<VirtualAddress>) {
        self.machine.invalidate(self.asid, address_range);
    }

    fn fence_all(&self) {
        self.machine.invalidate_all(self.asid);
    }

    unsafe fn read<T>(&self, address: VirtualAddress) -> T {
        unsafe { self.machine.read(self.asid, address) }
    }

    unsafe fn write<T>(&self, address: VirtualAddress, value: T) {
        unsafe { self.machine.write(self.asid, address, value) }
    }

    unsafe fn write_bytes(&self, address: VirtualAddress, value: u8, count: usize) {
        self.machine.write_bytes(self.asid, address, value, count)
    }
}

pub struct Machine<A: Arch, R: lock_api::RawMutex> {
    memory: Mutex<R, Memory>,
    memory_mode: &'static MemoryMode,
    cpu: CpuLocal<RefCell<Cpu<A>>>,
}

impl<A: Arch, R: lock_api::RawMutex> fmt::Debug for Machine<A, R>
where
    A::PageTableEntry: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Machine")
            .field("memory", &self.memory)
            .field("current_cpu", &self.cpu)
            .finish()
    }
}

impl<A: Arch, R: lock_api::RawMutex> Machine<A, R> {
    pub unsafe fn read<T>(&self, asid: u16, addr: VirtualAddress) -> T {
        assert!(addr.is_aligned_to(size_of::<T>()));

        let cpu = self.cpu.get().unwrap().borrow();
        if let Some((phys, attrs, _level)) = cpu.translate(asid, addr) {
            assert!(attrs.allows_read());

            unsafe { self.read_phys(phys) }
        } else {
            core::panic!("read: {addr} size {:#x} not present", size_of::<T>());
        }
    }

    pub unsafe fn write<T>(&self, asid: u16, addr: VirtualAddress, value: T) {
        assert!(addr.is_aligned_to(size_of::<T>()));

        let cpu = self.cpu.get().unwrap().borrow();
        if let Some((phys, attrs, _level)) = cpu.translate(asid, addr) {
            assert!(attrs.allows_read());

            unsafe { self.write_phys(phys, value) }
        } else {
            core::panic!("write: {addr} size {:#x} not present", size_of::<T>());
        }
    }

    pub fn write_bytes(&self, asid: u16, address: VirtualAddress, value: u8, count: usize) {
        let mut bytes_remaining = count;
        let mut address = address;

        let cpu = self.cpu.get().unwrap().borrow();
        while bytes_remaining > 0 {
            if let Some((phys, attrs, level)) = cpu.translate(asid, address) {
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
        unsafe { self.memory.lock().read(address) }
    }

    pub unsafe fn write_phys<T>(&self, address: PhysicalAddress, value: T) {
        unsafe { self.memory.lock().write(address, value) }
    }

    pub fn write_bytes_phys(&self, address: PhysicalAddress, value: u8, count: usize) {
        self.memory.lock().write_bytes(address, value, count)
    }

    pub fn memory_mode(&self) -> &'static MemoryMode {
        self.memory_mode
    }

    pub fn memory_regions<const MAX: usize>(&self) -> ArrayVec<Range<PhysicalAddress>, MAX> {
        self.memory
            .lock()
            .regions
            .iter()
            .map(|(end, (start, _))| *start..*end)
            .collect()
    }

    pub fn active_table(&self) -> Option<PhysicalAddress> {
        self.cpu.get().unwrap().borrow().active_page_table()
    }

    pub unsafe fn set_active_table(&self, address: PhysicalAddress) {
        self.cpu
            .get()
            .unwrap()
            .borrow_mut()
            .set_active_page_table(address);
    }

    pub fn invalidate(&self, asid: u16, address_range: Range<VirtualAddress>) {
        let mut cpu = self.cpu.get().unwrap().borrow_mut();
        let memory = self.memory.lock();

        cpu.invalidate(asid, address_range, &memory);
    }

    pub fn invalidate_all(&self, asid: u16) {
        let mut cpu = self.cpu.get().unwrap().borrow_mut();
        let memory = self.memory.lock();

        cpu.invalidate_all(asid, &memory);
    }
}

struct Cpu<A: Arch> {
    map: BTreeMap<
        (u16, VirtualAddress),
        (VirtualAddress, A::PageTableEntry, &'static PageTableLevel),
    >,
    page_table: Option<PhysicalAddress>,
    memory_mode: &'static MemoryMode,
}

impl<A: Arch> fmt::Debug for Cpu<A>
where
    A::PageTableEntry: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cpu")
            .field("map", &self.map)
            .field("page_table", &self.page_table)
            .field("memory_mode", &self.memory_mode)
            .finish()
    }
}

impl<A: Arch> Cpu<A> {
    pub fn new(memory_mode: &'static MemoryMode) -> Self {
        Cpu {
            map: BTreeMap::new(),
            page_table: None,
            memory_mode,
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

        // if let Some(page_table) = self.page_table {
        self.reload_map(asid, range, 0, self.page_table.unwrap(), memory);
        // }
    }

    pub fn invalidate_all(&mut self, asid: u16, memory: &Memory) {
        self.map.clear();

        // if let Some(page_table) = self.page_table {
        self.reload_map(
            asid,
            VirtualAddress::MIN..VirtualAddress::MAX.align_down(self.memory_mode.page_size()),
            0,
            self.page_table.unwrap(),
            memory,
        );
        // }
    }

    fn reload_map(
        &mut self,
        asid: u16,
        range: Range<VirtualAddress>,
        depth: u8,
        table: PhysicalAddress,
        memory: &Memory,
    ) {
        let level = &self.memory_mode.levels()[depth as usize];

        log::trace!("reloading map chunk {range:?}");
        let entries = page_table_entries_for(range, level, self.memory_mode);

        for (pte_index, range) in entries {
            let entry = unsafe {
                memory.read::<A::PageTableEntry>(
                    table.add(pte_index * size_of::<A::PageTableEntry>()),
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

pub struct Memory {
    regions: BTreeMap<PhysicalAddress, (PhysicalAddress, Box<[u8]>)>,
}

impl fmt::Debug for Memory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Memory")
            .field_with("regions", |f| {
                f.debug_list()
                    .entries(self.regions.iter().map(|(end, (start, _))| *start..*end))
                    .finish()
            })
            .finish()
    }
}

impl Memory {
    pub fn new(region_sizes: impl IntoIterator<Item = usize>, memory_mode: &MemoryMode) -> Self {
        let regions = region_sizes
            .into_iter()
            .map(|size| {
                let layout = Layout::from_size_align(size, memory_mode.page_size()).unwrap();
                let ptr = unsafe { std::alloc::alloc(layout) };
                let region: Box<[u8]> =
                    unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(ptr, size)) };

                let Range { start, end } = region.as_ptr_range();

                (
                    PhysicalAddress::from_ptr(end),
                    (PhysicalAddress::from_ptr(start), region),
                )
            })
            .collect();

        Self { regions }
    }

    pub fn get_region_containing(&self, address: PhysicalAddress) -> Option<(&[u8], usize)> {
        let (_end, (start, region)) = self.regions.range(address..).next()?;
        let offset = address.offset_from_unsigned(*start);
        Some((region, offset))
    }

    pub fn get_region_containing_mut(
        &mut self,
        address: PhysicalAddress,
    ) -> Option<(&mut [u8], usize)> {
        let (_end, (start, region)) = self.regions.range_mut(address..).next()?;
        let offset = address.get().checked_sub(start.get())?;
        Some((region, offset))
    }

    pub unsafe fn read<T>(&self, address: PhysicalAddress) -> T {
        let size = size_of::<T>();
        if let Some((region, offset)) = self.get_region_containing(address)
            && offset + size <= region.len()
        {
            unsafe { region.as_ptr().add(offset).cast::<T>().read() }
        } else {
            core::panic!("Memory::read: {address} size {size:#x} outside of memory ({self:?})");
        }
    }

    pub unsafe fn write<T>(&mut self, address: PhysicalAddress, value: T) {
        let size = size_of::<T>();
        if let Some((region, offset)) = self.get_region_containing_mut(address)
            && offset + size <= region.len()
        {
            unsafe { region.as_mut_ptr().add(offset).cast::<T>().write(value) };
        } else {
            core::panic!("Memory::write: {address} size {size:#x} outside of memory ({self:?})");
        }
    }

    pub fn write_bytes(&mut self, address: PhysicalAddress, value: u8, count: usize) {
        if let Some((region, offset)) = self.get_region_containing_mut(address)
            && offset + count <= region.len()
        {
            region[offset..offset + count].fill(value);
        } else {
            core::panic!(
                "Memory::write_bytes: {address} size {count:#x} outside of memory ({self:?})"
            );
        }
    }
}

pub struct MissingMemory;
pub struct HasMemory;
pub struct MissingMode;
pub struct HasMode;
pub struct MissingCpus;
pub struct HasCpus;

pub struct MachineBuilder<A: Arch, Mem, Mode, Cpus> {
    memory: Option<Memory>,
    memory_mode: Option<&'static MemoryMode>,
    cpu: CpuLocal<RefCell<Cpu<A>>>,
    _has: PhantomData<(Mem, Mode, Cpus)>,
}

impl<A: Arch> MachineBuilder<A, MissingMemory, MissingMode, MissingCpus> {
    pub const fn new() -> Self {
        Self {
            memory: None,
            memory_mode: None,
            cpu: CpuLocal::new(),
            _has: PhantomData,
        }
    }
}

impl<A: Arch, Cpus> MachineBuilder<A, MissingMemory, HasMode, Cpus> {
    pub fn with_memory_regions(
        self,
        region_sizes: impl IntoIterator<Item = usize>,
    ) -> MachineBuilder<A, HasMemory, HasMode, Cpus> {
        MachineBuilder {
            memory: Some(Memory::new(region_sizes, self.memory_mode.unwrap())),
            memory_mode: self.memory_mode,
            cpu: self.cpu,
            _has: PhantomData,
        }
    }
}

impl<A: Arch, Mem, Cpus> MachineBuilder<A, Mem, MissingMode, Cpus> {
    pub fn with_memory_mode(
        self,
        memory_mode: &'static MemoryMode,
    ) -> MachineBuilder<A, Mem, HasMode, Cpus> {
        MachineBuilder {
            memory: self.memory,
            memory_mode: Some(memory_mode),
            cpu: self.cpu,
            _has: PhantomData,
        }
    }
}

impl<A: Arch, Mem> MachineBuilder<A, Mem, HasMode, MissingCpus> {
    pub fn with_cpus(self, number_of_cpus: usize) -> MachineBuilder<A, Mem, HasMode, HasCpus> {
        let mut cpu = CpuLocal::with_capacity(number_of_cpus);

        for cpuid in 0..number_of_cpus {
            cpu.insert_for(cpuid, RefCell::new(Cpu::new(self.memory_mode.unwrap())));
        }

        MachineBuilder {
            memory: self.memory,
            memory_mode: self.memory_mode,
            cpu,
            _has: PhantomData,
        }
    }
}

impl<A: Arch> MachineBuilder<A, HasMemory, HasMode, HasCpus> {
    pub fn finish<R: lock_api::RawMutex>(self) -> Machine<A, R> {
        Machine {
            memory: Mutex::new(self.memory.unwrap()),
            memory_mode: self.memory_mode.unwrap(),
            cpu: self.cpu,
        }
    }
}
