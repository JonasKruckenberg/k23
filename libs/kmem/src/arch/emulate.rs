use core::ops::Range;
use core::fmt;
use std::mem;
use crate::arch::{Arch, PageTableLevel};
use crate::{
    PhysicalAddress
    , VirtualAddress,
};
use crate::test_utils::Machine;

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
    pub const fn new(machine: Machine<A, R>) -> Self {
        Self::with_asid(machine, 0)
    }

    pub const fn with_asid(machine: Machine<A, R>, asid: u16) -> Self {
        Self { machine, asid }
    }

    pub const fn machine(&self) -> &Machine<A, R> {
        &self.machine
    }
}

impl<A: Arch, R: lock_api::RawMutex> Arch for EmulateArch<A, R> {
    type PageTableEntry = A::PageTableEntry;

    const LEVELS: &'static [PageTableLevel] = A::LEVELS;
    const DEFAULT_PHYSMAP_BASE: VirtualAddress = A::DEFAULT_PHYSMAP_BASE;

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
        if self.active_table().is_some() {
            unsafe { self.machine.read(self.asid, address) }
        } else {
            let address = unsafe { mem::transmute::<VirtualAddress, PhysicalAddress>(address) };
            unsafe { self.machine.read_phys(address) }
        }
    }

    unsafe fn write<T>(&self, address: VirtualAddress, value: T) {
        if self.active_table().is_some() {
            unsafe { self.machine.write(self.asid, address, value) }
        } else {
            let address = unsafe { mem::transmute::<VirtualAddress, PhysicalAddress>(address) };
            unsafe { self.machine.write_phys(address, value) }
        }
    }

    unsafe fn write_bytes(&self, address: VirtualAddress, value: u8, count: usize) {
        if self.active_table().is_some() {
            self.machine.write_bytes(self.asid, address, value, count)
        } else {
            let address = unsafe { mem::transmute::<VirtualAddress, PhysicalAddress>(address) };
            self.machine.write_bytes_phys(address, value, count)
        }
    }
}

