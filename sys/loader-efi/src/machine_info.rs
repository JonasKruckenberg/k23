// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::cell::LazyCell;
use core::slice;

use fdt::Fdt;
use loader_api::FirmwareTables;
use loader_common::MachineInfo;
use mem_core::PhysicalAddress;
use uefi::boot::AllocateType;
use uefi::mem::memory_map::MemoryType;
use uefi::proto::unsafe_protocol;
use uefi::table::cfg::ConfigTableEntry;
use uefi::{Guid, Status, StatusExt, boot, guid};

use crate::Error;

/// UEFI config-table GUID for a flattened DTB. Not defined in `uefi-raw`;
/// see [riscv-non-isa/riscv-uefi].
///
/// [riscv-non-isa/riscv-uefi]: https://github.com/riscv-non-isa/riscv-uefi/blob/main/boot_protocol.adoc
const EFI_DTB_TABLE_GUID: Guid = guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0");

/// Build a [`MachineInfo`] by scanning the UEFI config table and calling the
/// relevant protocols / per-arch hooks.
pub(crate) fn discover() -> crate::Result<MachineInfo> {
    let firmware_tables = discover_firmware_tables()?;

    let fdt = firmware_tables.raw_fdt.map(open_fdt).transpose()?;
    let chosen = LazyCell::new(|| fdt.as_ref()?.find_node("/chosen").ok().flatten());

    Ok(MachineInfo {
        // try to obtain the boot hart ID from UEFI first, then fall back to the FDT
        boot_hart_id: efi_boot_hartid()
            .or_else(|| fdt.as_ref().and_then(loader_common::fdt::boot_hartid))
            .ok_or(Error::NoBootHartId)?,
        // try to obtain the RNG seed from UEFI first, then fall back to FDT
        rng_seed: efi_rng_seed()
            .or_else(|| chosen.as_ref().and_then(loader_common::fdt::rng_seed))
            .ok_or(Error::NoRngSeed)?,
        // the only mechanism we have to discover the UART port is through FDT...
        uart: fdt.as_ref().zip(chosen.as_ref()).and_then(|(fdt, chosen)| {
            loader_common::fdt::stdout_uart(fdt, chosen, uefi::boot::PAGE_SIZE)
        }),
        firmware_tables,
    })
}

fn discover_firmware_tables() -> crate::Result<FirmwareTables> {
    let mut raw_rsdp = None;
    let mut raw_fdt = None;
    let mut raw_smbios3 = None;
    uefi::system::with_config_table(|entries| {
        for e in entries {
            let addr = PhysicalAddress::from_ptr(e.address);
            match e.guid {
                ConfigTableEntry::ACPI2_GUID => raw_rsdp = Some(addr),
                ConfigTableEntry::ACPI_GUID if raw_rsdp.is_none() => raw_rsdp = Some(addr),
                ConfigTableEntry::SMBIOS3_GUID => raw_smbios3 = Some(addr),
                EFI_DTB_TABLE_GUID => raw_fdt = Some(addr),
                other => log::trace!("ignoring UEFI config table {other}"),
            }
        }
    });

    // NB: EDK2 has a fun bug where the FDT and SMBIOS are placed right below the EFI apps stack _without_
    // any guard pages or protection. This means as our stack grows we can override the FDT and SMBIOS blobs...
    // (the reserved stack is quite small too, so in debug mode we've actually hit this regularly)
    // To work around this we copy the FDT and SMBIOS blobs into fresh allocations. This has the advantage of
    // BOTH correctly tracking the allocation as `RESERVED` AND drawing from the firmwares high DXE pool
    // (well clear of our stack).
    Ok(FirmwareTables {
        // ACPI RSDP does not need staging, UEFI already correctly tracks and manages this one...
        raw_rsdp,
        raw_fdt: raw_fdt.map(stage_fdt).transpose()?,
        raw_smbios3: raw_smbios3.map(stage_smbios3).transpose()?,
    })
}

/// Copy the FDT into loader-owned memory.
fn stage_fdt(addr: PhysicalAddress) -> crate::Result<PhysicalAddress> {
    assert_eq!(addr.get() % align_of::<u32>(), 0, "FDT not u32-aligned");

    #[expect(clippy::cast_ptr_alignment, reason = "aligned is checked above")]
    // Safety: `discover` already parsed this same u32-aligned pointer.
    let fdt = unsafe { Fdt::from_ptr(addr.as_ptr().cast::<u32>()) }?;
    stage_blob(&fdt.as_slice()[..fdt.total_size()])
}

/// Stage the SMBIOS 3.0 entry point together with its structure table.
fn stage_smbios3(ep_addr: PhysicalAddress) -> crate::Result<PhysicalAddress> {
    const EP_LEN: usize = 0x18; // SMBIOS 3.0 entry point is 24 bytes (DSP0134 §5.2.2)

    // Safety: the SMBIOS3 config table points at a valid 24-byte entry point.
    let ep = unsafe { slice::from_raw_parts(ep_addr.as_ptr(), EP_LEN) };
    if &ep[..5] != b"_SM3_" {
        return Err(Error::BadSmbios);
    }

    // §5.2.2: structure-table max size at 0x0C (u32), table address at 0x10 (u64).
    let table_len = u32::from_le_bytes(ep[0x0C..0x10].try_into().unwrap()) as usize;
    let table_addr = u64::from_le_bytes(ep[0x10..0x18].try_into().unwrap());
    // Safety: the entry point describes a `table_len`-byte structure table at `table_addr`.
    let table = unsafe { slice::from_raw_parts(table_addr as *const u8, table_len) };
    let staged_table = stage_blob(table)?;

    // Rebuild the entry point pointing at the staged table, then fix the checksum
    // so the whole structure sums to zero (mod 256).
    let mut staged_ep = [0u8; EP_LEN];
    staged_ep.copy_from_slice(ep);
    staged_ep[0x10..0x18].copy_from_slice(&(staged_table.get() as u64).to_le_bytes());
    staged_ep[0x05] = 0;
    let sum = staged_ep.iter().fold(0u8, |acc, &b| acc.wrapping_add(b));
    staged_ep[0x05] = sum.wrapping_neg();

    stage_blob(&staged_ep)
}

/// Copy `bytes` into freshly-allocated `RESERVED` pages and return the copy's
/// physical address.
fn stage_blob(bytes: &[u8]) -> crate::Result<PhysicalAddress> {
    let pages = bytes.len().div_ceil(uefi::boot::PAGE_SIZE).max(1);
    let base = boot::allocate_pages(AllocateType::AnyPages, MemoryType::RESERVED, pages)?;
    // Safety: `allocate_pages` returned `pages` (≥ `bytes.len()`) of fresh, never-freed
    // memory, identity-mapped while boot services are live.
    let dst = unsafe { slice::from_raw_parts_mut(base.as_ptr(), bytes.len()) };
    dst.copy_from_slice(bytes);
    Ok(PhysicalAddress::from_ptr(base.as_ptr().cast_const()))
}

fn open_fdt(raw_fdt: PhysicalAddress) -> crate::Result<Fdt<'static>> {
    assert!(
        raw_fdt.is_aligned_to(align_of::<u32>()),
        "FDT not u32-aligned"
    );

    #[expect(clippy::cast_ptr_alignment, reason = "aligned is checked above")]
    // Safety: caller guarantees validity and lifetime.
    unsafe { Fdt::from_ptr(raw_fdt.as_ptr().cast::<u32>()) }.map_err(Into::into)
}

/// Read 32 bytes from `EFI_RNG_PROTOCOL`. Returns `None` if the protocol is
/// absent or any call fails — caller falls back to `/chosen/rng-seed`.
fn efi_rng_seed() -> Option<[u8; 32]> {
    use uefi::boot;
    use uefi::proto::rng::Rng;

    let handle = boot::get_handle_for_protocol::<Rng>().ok()?;
    let mut rng = boot::open_protocol_exclusive::<Rng>(handle).ok()?;
    let mut seed = [0u8; 32];
    rng.get_rng(None, &mut seed).ok()?;
    Some(seed)
}

fn efi_boot_hartid() -> Option<usize> {
    // NB: the upstream uefi crate does not have support for the riscv protocol
    // so we have our own ad-hoc bindings here.

    /// Not in uefi-rs/uefi-raw — defined inline here.
    /// GUID `ccd15fec-6f73-4eec-8395-3e69e4b940bf`.
    #[repr(C)]
    #[unsafe_protocol("ccd15fec-6f73-4eec-8395-3e69e4b940bf")]
    struct RiscvEfiBootProtocol {
        revision: u64,
        get_boot_hartid: unsafe extern "efiapi" fn(this: *mut Self, hartid: *mut usize) -> Status,
    }

    let handle = boot::get_handle_for_protocol::<RiscvEfiBootProtocol>().ok()?;
    let mut proto = boot::open_protocol_exclusive::<RiscvEfiBootProtocol>(handle).ok()?;

    let mut hartid: usize = 0;

    // Safety: EFI firmware call
    unsafe { (proto.get_boot_hartid)(&mut *proto, &mut hartid) }
        .to_result()
        .ok()?;

    Some(hartid)
}
