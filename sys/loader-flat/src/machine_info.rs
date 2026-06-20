// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::cell::LazyCell;
use core::range::Range;

use arrayvec::ArrayVec;
use fdt::Fdt;
use loader_api::{FirmwareTables, MemoryRegion, MemoryRegionKind};
use loader_common::MachineInfo;
use mem_core::PhysicalAddress;

use crate::Error;

pub(crate) fn from_dtb<const N: usize>(
    raw_fdt: PhysicalAddress,
    boot_hart_id: usize,
    granule_size: usize,
) -> crate::Result<(MachineInfo, ArrayVec<MemoryRegion, N>)> {
    assert!(
        raw_fdt.is_aligned_to(align_of::<u32>()),
        "FDT not u32-aligned"
    );

    #[expect(clippy::cast_ptr_alignment, reason = "aligned is checked above")]
    // Safety: caller guarantees validity and lifetime.
    let fdt = unsafe { Fdt::from_ptr(raw_fdt.as_ptr().cast::<u32>())? };

    let chosen = LazyCell::new(|| fdt.find_node("/chosen").ok().flatten());

    log::info!(
        "FDT BOOT HART {:?} SBI BOOT HART {}",
        loader_common::fdt::boot_hartid(&fdt),
        boot_hart_id
    );

    let minfo = MachineInfo {
        // try to obtain the boot hart ID from UEFI first, then fall back to the FDT
        boot_hart_id,
        // try to obtain the RNG seed from UEFI first, then fall back to FDT
        rng_seed: loader_common::fdt::rng_seed(chosen.as_ref().unwrap()).ok_or(Error::NoRngSeed)?,
        // the only mechanism we have to discover the UART port is through FDT...
        uart: loader_common::fdt::stdout_uart(&fdt, chosen.as_ref().unwrap(), granule_size),
        firmware_tables: FirmwareTables {
            raw_fdt: Some(raw_fdt),
            raw_rsdp: None,
            raw_smbios3: None,
        },
    };

    let mut memory_regions = loader_common::fdt::physical_memory_regions(&fdt)?;

    // make sure the FDT is excluded so we don't accidentally override it with
    apply_fdt_reservation(&mut memory_regions, &fdt)?;

    // make sure the loader itself is also excluded, this will be no fun otherwise
    apply_loader_reservation(&mut memory_regions, granule_size)?;

    Ok((minfo, memory_regions))
}

fn apply_fdt_reservation<const N: usize>(
    regions: &mut ArrayVec<MemoryRegion, N>,
    fdt: &Fdt,
) -> crate::Result<()> {
    let fdt_phys = {
        let range = fdt.as_slice().as_ptr_range();
        Range::from(PhysicalAddress::from_ptr(range.start)..PhysicalAddress::from_ptr(range.end))
    };
    loader_common::fdt::apply_reservation(
        regions,
        fdt_phys,
        MemoryRegionKind::FirmwareTableReclaimable,
    )?;

    Ok(())
}

fn apply_loader_reservation<const N: usize>(
    regions: &mut ArrayVec<MemoryRegion, N>,
    granule_size: usize,
) -> crate::Result<()> {
    let loader_phys = {
        unsafe extern "C" {
            static __loader_start: u8;
            static __loader_end: u8;
        }

        let start = PhysicalAddress::from_ptr(&raw const __loader_start);
        let end = PhysicalAddress::from_ptr(&raw const __loader_end);

        assert!(start.is_aligned_to(granule_size));
        assert!(end.is_aligned_to(granule_size));

        Range::from(start..end)
    };
    loader_common::fdt::apply_reservation(
        regions,
        loader_phys,
        MemoryRegionKind::LoaderReclaimable,
    )?;

    Ok(())
}
