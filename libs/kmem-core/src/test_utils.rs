// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch::emulate::{EmulateArch, MachineBuilder};
use crate::bootstrap::BootstrapAllocator;
use crate::frame_alloc::FrameAllocator;
use crate::{AddressSpace, Arch, Flush};

pub(crate) fn setup_aspace_and_alloc<A: Arch>(
    arch: A,
    region_sizes: impl IntoIterator<Item = usize>,
) -> (
    AddressSpace<EmulateArch<A, parking_lot::RawMutex>>,
    impl FrameAllocator,
) {
    let machine = MachineBuilder::new()
        .with_memory_mode(arch.memory_mode())
        .with_memory_regions(region_sizes)
        .with_cpus(1)
        .finish();

    let arch: EmulateArch<A, parking_lot::RawMutex> = EmulateArch::new(machine);

    let frame_alloc: BootstrapAllocator<parking_lot::RawMutex> = BootstrapAllocator::new(
        arch.machine().memory_regions(),
        arch.memory_mode().page_size(),
    );

    let mut flush = Flush::new();
    let mut aspace = AddressSpace::new_bootstrap(arch, frame_alloc.by_ref(), &mut flush).unwrap();

    aspace
        .map_physical_memory(&frame_alloc, &mut flush)
        .unwrap();

    // Safety: we just created the address space, so don't have any pointers into it. In hosted tests
    // the programs memory and CPU registers are outside the address space anyway.
    let aspace = unsafe { aspace.finish_bootstrap_and_activate() };

    flush.flush(aspace.arch());

    (aspace, frame_alloc)
}
