use core::fmt;
use core::ops::Range;
use std::mem;

use crate::arch::{Arch, PageTableLevel};
use crate::test_utils::Machine;
use crate::{PhysicalAddress, VirtualAddress};

/// `[Arch`] implementation that emulates a given "real" architecture. For testing purposes.
pub struct EmulateArch<A: Arch> {
    machine: Machine<A>,
    asid: u16,
}

impl<A: Arch> fmt::Debug for EmulateArch<A>
where
    A::PageTableEntry: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmulateArch")
            .field("machine", &self.machine)
            .finish()
    }
}

impl<A: Arch> EmulateArch<A> {
    pub const fn new(machine: Machine<A>) -> Self {
        Self::with_asid(machine, 0)
    }

    pub const fn with_asid(machine: Machine<A>, asid: u16) -> Self {
        Self { machine, asid }
    }

    pub const fn machine(&self) -> &Machine<A> {
        &self.machine
    }
}

impl<A: Arch> Arch for EmulateArch<A> {
    // We want to inherit all const parameters from the proper architecture...

    type PageTableEntry = A::PageTableEntry;
    const LEVELS: &'static [PageTableLevel] = A::LEVELS;
    const DEFAULT_PHYSMAP_BASE: VirtualAddress = A::DEFAULT_PHYSMAP_BASE;

    // ...while we emulate all other methods.

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
        // NB: if there is no active page table on this CPU, we are in "bare" translation mode.
        // In which case we need to use `read_phys` instead of `read`, bypassing
        // translation checks.
        if self.active_table().is_some() {
            unsafe { self.machine.read(self.asid, address) }
        } else {
            // Safety: We checked for the absence of an active translation table, meaning we're in
            // "bare" mode and VirtualAddress==PhysicalAddress.
            let address = unsafe { mem::transmute::<VirtualAddress, PhysicalAddress>(address) };
            unsafe { self.machine.read_phys(address) }
        }
    }

    unsafe fn write<T>(&self, address: VirtualAddress, value: T) {
        // NB: if there is no active page table on this CPU, we are in "bare" translation mode.
        // In which case we need to use `write_phys` instead of `write`, bypassing
        // translation checks.
        if self.active_table().is_some() {
            unsafe { self.machine.write(self.asid, address, value) }
        } else {
            // Safety: We checked for the absence of an active translation table, meaning we're in
            // "bare" mode and VirtualAddress==PhysicalAddress.
            let address = unsafe { mem::transmute::<VirtualAddress, PhysicalAddress>(address) };
            unsafe { self.machine.write_phys(address, value) }
        }
    }

    unsafe fn read_bytes(&self, address: VirtualAddress, count: usize) -> &[u8] {
        // NB: if there is no active page table on this CPU, we are in "bare" translation mode.
        // In which case we need to use `write_bytes_phys` instead of `write_bytes`, bypassing
        // translation checks.
        if self.active_table().is_some() {
            self.machine.read_bytes(self.asid, address, count)
        } else {
            // Safety: We checked for the absence of an active translation table, meaning we're in
            // "bare" mode and VirtualAddress==PhysicalAddress.
            let address = unsafe { mem::transmute::<VirtualAddress, PhysicalAddress>(address) };
            self.machine.read_bytes_phys(address, count)
        }
    }

    unsafe fn write_bytes(&self, address: VirtualAddress, value: u8, count: usize) {
        // NB: if there is no active page table on this CPU, we are in "bare" translation mode.
        // In which case we need to use `write_bytes_phys` instead of `write_bytes`, bypassing
        // translation checks.
        if self.active_table().is_some() {
            self.machine.write_bytes(self.asid, address, value, count)
        } else {
            // Safety: We checked for the absence of an active translation table, meaning we're in
            // "bare" mode and VirtualAddress==PhysicalAddress.
            let address = unsafe { mem::transmute::<VirtualAddress, PhysicalAddress>(address) };
            self.machine.write_bytes_phys(address, value, count)
        }
    }
}
