// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::slice;

use fallible_iterator::FallibleIterator;
use fdt::{CellSizes, Fdt};
use mem_core::PhysicalAddress;
use uefi::boot::{self, AllocateType, MemoryType};
use uefi::table::cfg::ConfigTableEntry;
use uefi::{Guid, guid};

use crate::error::Error;
use crate::{Result, arch};

/// UEFI config-table GUID for a flattened DTB. Not defined in `uefi-raw`;
/// see [riscv-non-isa/riscv-uefi].
///
/// [riscv-non-isa/riscv-uefi]: https://github.com/riscv-non-isa/riscv-uefi/blob/main/boot_protocol.adoc
const EFI_DTB_TABLE_GUID: Guid = guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0");

#[derive(Debug)]
pub(crate) struct MachineInfo {
    /// Firmware-reported id of the boot CPU.
    ///
    /// - riscv64: hartid (from `RISCV_EFI_BOOT_PROTOCOL`, fallback `/chosen/boot-hartid`)
    /// - aarch64: `MPIDR_EL1` affinity bits
    /// - x86_64:  x2APIC id (from `cpuid`)
    pub boot_hart_id: usize,
    /// 32-byte seed for the kernel PRNG / KASLR.
    /// Source: `EFI_RNG_PROTOCOL`, fallback `/chosen/rng-seed`.
    pub rng_seed: [u8; 32],
    /// ACPI RSDP — kernel walks the XSDT for MADT, SRAT, SLIT, …. Firmware
    /// pointer, passed through verbatim (not staged — see [`Self::stage_tables`]).
    pub raw_rsdp: Option<PhysicalAddress>,
    /// FDT blob — kernel walks `/cpus`, `/memory`, …. Points at the loader-owned
    /// copy once [`Self::stage_tables`] has run.
    pub raw_fdt: Option<PhysicalAddress>,
    /// SMBIOS3 entry point — opaque to the loader. Points at the loader-owned
    /// copy once [`Self::stage_tables`] has run.
    pub raw_smbios3: Option<PhysicalAddress>,
    pub uart: Option<PhysicalAddress>,
}

impl MachineInfo {
    /// Relocate the flat firmware tables into loader-owned `RESERVED` memory and
    /// repoint at the copies.
    ///
    /// QEMU's EDK2 leaves the FDT in `EfiBootServicesData` carved from the same
    /// low pool as the loader's UEFI stack, so it is both overrun by our own
    /// stack during deep UEFI calls and reclaimed at `ExitBootServices`. Like
    /// Linux's EFI stub we copy the tables out *early* — while the stack is
    /// shallow and boot services are live — so the kernel receives stable copies.
    ///
    /// ACPI is left untouched: its RSDP→XSDT→… graph would need a deep copy with
    /// checksum fixups, and on this firmware it sits in preserved
    /// `EfiACPIReclaimMemory` far from the stack. TODO: stage it too.
    ///
    /// # Errors
    ///
    /// Returns an error if an allocation fails, the FDT no longer parses, or the
    /// SMBIOS entry point is malformed.
    pub(crate) fn stage_tables(&mut self) -> Result<()> {
        if let Some(fdt) = self.raw_fdt {
            self.raw_fdt = Some(stage_fdt(fdt)?);
        }
        if let Some(smbios3) = self.raw_smbios3 {
            self.raw_smbios3 = Some(stage_smbios3(smbios3)?);
        }
        Ok(())
    }
}

/// Copy the FDT into loader-owned memory. The firmware blob is still intact here
/// (no deep-stack UEFI work has run), so we re-parse it to recover its length.
fn stage_fdt(addr: PhysicalAddress) -> Result<PhysicalAddress> {
    // Safety: `discover` already parsed this same u32-aligned pointer.
    let fdt = unsafe { Fdt::from_ptr(addr.as_ptr().cast::<u32>()) }?;
    stage_blob(&fdt.as_slice()[..fdt.total_size()])
}

/// Copy `bytes` into freshly-allocated `RESERVED` pages and return the copy's
/// physical address. `RESERVED` memory is preserved across `ExitBootServices`,
/// and `allocate_pages` draws from the high DXE pool — away from the loader
/// stack — so the copy escapes both hazards the firmware placement exposes.
fn stage_blob(bytes: &[u8]) -> Result<PhysicalAddress> {
    let pages = bytes.len().div_ceil(uefi::boot::PAGE_SIZE).max(1);
    let base = boot::allocate_pages(AllocateType::AnyPages, MemoryType::RESERVED, pages)?;
    // Safety: `allocate_pages` returned `pages` (≥ `bytes.len()`) of fresh, never-freed
    // memory, identity-mapped while boot services are live.
    let dst = unsafe { slice::from_raw_parts_mut(base.as_ptr(), bytes.len()) };
    dst.copy_from_slice(bytes);
    Ok(PhysicalAddress::from_ptr(base.as_ptr().cast_const()))
}

/// Stage the SMBIOS 3.0 entry point together with its structure table.
///
/// The `_SM3_` entry point (DSP0134 §5.2.2) points at a *separate* structure
/// table, so we relocate both, repoint the entry point at the staged table, and
/// recompute the entry-point checksum.
fn stage_smbios3(ep_addr: PhysicalAddress) -> Result<PhysicalAddress> {
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

/// Build a [`MachineInfo`] by scanning the UEFI config table and calling the
/// relevant protocols / per-arch hooks.
pub(crate) fn discover() -> Result<MachineInfo> {
    // Scan SystemTable.ConfigurationTable. Prefer ACPI 2.0+ over 1.0; ignore
    // everything else (ESRT, MemoryAttributesTable, legacy SMBIOS, …).
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

    // Open the FDT once. Reused for the boot-CPU fallback (rv64) and the
    // rng-seed fallback.
    let fdt = raw_fdt
        .map(|addr| {
            assert_eq!(addr.get() % align_of::<u32>(), 0, "FDT not u32-aligned");
            // Safety: caller guarantees validity and lifetime.
            unsafe { Fdt::from_ptr(addr.as_ptr().cast::<u32>()) }.map_err(Error::from)
        })
        .transpose()?;

    let boot_hart_id = arch::boot_hart_id(fdt.as_ref())?;

    let rng_seed = efi_rng_seed()
        .or_else(|| fdt.as_ref().and_then(fdt_rng_seed))
        .ok_or(Error::NoRngSeed)?;

    if let Some(fdt) = fdt {
        let chosen = fdt.find_node("/chosen")?.unwrap();

        let stdout_path = chosen.find_property("stdout-path")?.unwrap();

        let uart = fdt.find_node(stdout_path.as_str()?)?.unwrap();

        let address_cells = fdt.find_property("#address-cells")?.unwrap().as_usize()?;
        let size_cells = fdt.find_property("#size-cells")?.unwrap().as_usize()?;

        let compatible = uart.find_property("compatible")?.unwrap().as_str()?;
        let reg = uart.find_property("reg")?.unwrap().as_regs(CellSizes {
            address_cells,
            size_cells,
        });

        uart.properties().for_each(|prop| {
            log::debug!("name {}", prop.name);
            Ok(())
        });

        log::debug!("chosen stdout compatible {compatible:?}");
    }

    Ok(MachineInfo {
        boot_hart_id,
        rng_seed,
        raw_rsdp,
        raw_fdt,
        raw_smbios3,
        uart: None,
    })
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

/// Read `/chosen/rng-seed`. Returns `None` if absent or shorter than 32
/// bytes — we won't seed ChaCha20 with a weak input.
fn fdt_rng_seed(fdt: &Fdt<'_>) -> Option<[u8; 32]> {
    let prop = fdt
        .find_node("/chosen")
        .ok()??
        .find_property("rng-seed")
        .ok()??;
    prop.raw.first_chunk::<32>().copied()
}
