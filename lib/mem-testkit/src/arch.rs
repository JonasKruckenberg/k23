// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
use core::range::Range;
use std::mem;

use mem_core::arch::{Arch, MapsAt, PageTableLevel};
use mem_core::{PageSize, PhysicalAddress, VirtualAddress};

use crate::machine::Machine;

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
            .field("asid", &self.asid)
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
        // Safety: ensured by caller
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
            // Safety: ensured by caller.
            unsafe { self.machine.read(self.asid, address) }
        } else {
            // Safety: We checked for the absence of an active translation table, meaning we're in
            // "bare" mode and VirtualAddress==PhysicalAddress.
            let address = unsafe { mem::transmute::<VirtualAddress, PhysicalAddress>(address) };
            // Safety: validity/alignment ensured by caller
            unsafe { self.machine.read_phys(address) }
        }
    }

    unsafe fn write<T>(&self, address: VirtualAddress, value: T) {
        // NB: if there is no active page table on this CPU, we are in "bare" translation mode.
        // In which case we need to use `write_phys` instead of `write`, bypassing
        // translation checks.
        if self.active_table().is_some() {
            // Safety: ensured by caller.
            unsafe { self.machine.write(self.asid, address, value) }
        } else {
            // Safety: We checked for the absence of an active translation table, meaning we're in
            // "bare" mode and VirtualAddress==PhysicalAddress.
            let address = unsafe { mem::transmute::<VirtualAddress, PhysicalAddress>(address) };
            // Safety: validity/alignment ensured by caller
            unsafe { self.machine.write_phys(address, value) }
        }
    }

    unsafe fn read_bytes(&self, address: VirtualAddress, count: usize) -> &[u8] {
        // NB: if there is no active page table on this CPU, we are in "bare" translation mode.
        // In which case we need to use `write_bytes_phys` instead of `write_bytes`, bypassing
        // translation checks.
        if self.active_table().is_some() {
            // Safety: ensured by caller.
            unsafe { self.machine.read_bytes(self.asid, address, count) }
        } else {
            // Safety: We checked for the absence of an active translation table, meaning we're in
            // "bare" mode and VirtualAddress==PhysicalAddress. All other safety invariants are
            // ensured by the caller.
            let address = unsafe { mem::transmute::<VirtualAddress, PhysicalAddress>(address) };
            // Safety: validity ensured by caller
            self.machine.read_bytes_phys(address, count)
        }
    }

    unsafe fn write_bytes(&self, address: VirtualAddress, value: u8, count: usize) {
        // NB: if there is no active page table on this CPU, we are in "bare" translation mode.
        // In which case we need to use `write_bytes_phys` instead of `write_bytes`, bypassing
        // translation checks.
        if self.active_table().is_some() {
            // Safety: ensured by caller.
            unsafe { self.machine.write_bytes(self.asid, address, value, count) }
        } else {
            // Safety: We checked for the absence of an active translation table, meaning we're in
            // "bare" mode and VirtualAddress==PhysicalAddress. All other safety invariants are
            // ensured by the caller.
            let address = unsafe { mem::transmute::<VirtualAddress, PhysicalAddress>(address) };
            // Safety: validity ensured by caller
            self.machine.write_bytes_phys(address, value, count);
        }
    }
}

/// Forward the emulated architecture's page-size capabilities: `EmulateArch<A>` maps a
/// leaf of size `S` at exactly the depth `A` does, so tests exercise the real arch's
/// [`MapsAt`] depths rather than a substitute.
impl<S: PageSize, A: MapsAt<S>> MapsAt<S> for EmulateArch<A> {
    const DEPTH: u8 = <A as MapsAt<S>>::DEPTH;
}
