// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::fmt;
use core::range::Range;

use arrayvec::ArrayVec;
use human_bytes::HumanBytes;
use mem_core::{PhysMap, PhysicalAddress, VirtualAddress};

pub const BOOT_INFO_VERSION: u32 = 1;
pub const MAX_MEMORY_REGIONS: usize = 128;

pub type MemoryRegions = ArrayVec<MemoryRegion, { MAX_MEMORY_REGIONS }>;

#[derive(Debug)]
#[non_exhaustive]
pub struct BootInfo {
    pub version: u32,
    pub boot_cpu_id: usize,
    pub boot_ticks: u64,

    pub fdt: Option<PhysicalAddress>,
    pub acpi_rsdp: Option<PhysicalAddress>,
    pub smbios3: Option<PhysicalAddress>,

    pub rng_seed: [u8; 32],

    /// A map of the physical memory regions of the underlying machine.
    ///
    /// The loader parses this information from the firmware and also reports regions used
    /// during initial mapping. Regions marked as usable can be freely
    /// used by the kernel.
    ///
    /// Note: Memory regions are *guaranteed* to not overlap and be sorted by their start address.
    /// But they might not be optimally packed, i.e. adjacent regions that could be merged are not.
    pub memory_regions: MemoryRegions,

    /// Virtual address of the loaded kernel image.
    pub kernel_virt: Range<VirtualAddress>,
    /// The thread local storage (TLS) template of the kernel executable.
    pub tls_template: TlsTemplate,
    /// Physical memory region where the kernel's debuginfo ELF resides.
    ///
    /// Contains the `.symtab` and `.debug_*` sections stripped from the runnable kernel
    /// image.
    pub kernel_debuginfo_phys: Option<Range<PhysicalAddress>>,

    /// Trampoline that was identity-mapped into the kernel address space for handoff.
    /// Can safely be unmapped and reused by the kernel.
    pub handoff_trampoline_virt: Range<VirtualAddress>,

    pub physmap: PhysMap,
}

impl BootInfo {
    /// Create a new boot info structure with the given memory map.
    ///
    /// The other fields are initialized with default values.
    pub fn new(physmap: PhysMap) -> Self {
        Self {
            version: BOOT_INFO_VERSION,
            boot_cpu_id: 0,
            boot_ticks: 0,
            fdt: None,
            acpi_rsdp: None,
            smbios3: None,
            rng_seed: [0; 32],
            memory_regions: ArrayVec::new(),
            kernel_virt: Range::default(),
            tls_template: TlsTemplate::default(),
            kernel_debuginfo_phys: None,
            handoff_trampoline_virt: Range::default(),
            physmap,
        }
    }
}

/// Represent a physical memory region.
#[derive(Debug, Clone, Eq, PartialEq)]
#[repr(C)]
pub struct MemoryRegion {
    /// The physical start address region.
    pub range: Range<PhysicalAddress>,
    /// The memory type of the memory region.
    ///
    /// Only [`Usable`][MemoryRegionKind::Usable] regions can be freely used.
    pub kind: MemoryRegionKind,
}

/// Represents the different types of memory.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[non_exhaustive]
#[repr(C)]
pub enum MemoryRegionKind {
    /// Memory in which errors have been detected or which is otherwise unusable.
    Unusable,
    /// Unused conventional memory, can be used by the kernel.
    Usable,
}

impl MemoryRegionKind {
    pub fn is_usable(&self) -> bool {
        matches!(self, MemoryRegionKind::Usable)
    }
}

impl fmt::Display for BootInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{:<23} : {:?}", "PHYSICAL MEMORY MAP", self.physmap)?;

        writeln!(f, "{:<23} : {}", "BOOT CPU ID", self.boot_cpu_id)?;
        writeln!(f, "{:<23} : {}", "BOOT TICKS", self.boot_ticks)?;

        if let Some(fdt) = &self.fdt {
            writeln!(f, "{:<23} : {}", "FDT", fdt)?;
        } else {
            writeln!(f, "{:<23} : None", "FDT")?;
        }

        if let Some(acpi_rsdp) = &self.acpi_rsdp {
            writeln!(f, "{:<23} : {}", "ACPI RDSP", acpi_rsdp)?;
        } else {
            writeln!(f, "{:<23} : None", "ACPI RDSP")?;
        }

        if let Some(smbios3) = &self.smbios3 {
            writeln!(f, "{:<23} : {}", "SMBIOS", smbios3)?;
        } else {
            writeln!(f, "{:<23} : None", "SMBIOS")?;
        }

        writeln!(
            f,
            "{:<23} : {}..{}",
            "KERNEL VIRT", self.kernel_virt.start, self.kernel_virt.end
        )?;

        if let Some(kernel_debuginfo_phys) = &self.kernel_debuginfo_phys {
            writeln!(
                f,
                "{:<23} : {}..{}",
                "KERNEL DEBUGINFO PHYS", kernel_debuginfo_phys.start, kernel_debuginfo_phys.end
            )?;
        } else {
            writeln!(f, "{:<23} : None", "KERNEL DEBUGINFO PHYS")?;
        }

        let tls = &self.tls_template;
        writeln!(
            f,
            "{:<23} : .tdata: {}..{}, .tbss: {}..{}",
            "TLS TEMPLATE",
            tls.image_offset,
            tls.image_offset + tls.file_size,
            tls.image_offset + tls.file_size,
            tls.image_offset + tls.file_size + tls.mem_size
        )?;
        for (idx, region) in self.memory_regions.iter().enumerate() {
            let size = region.range.end.offset_from_unsigned(region.range.start);

            writeln!(
                f,
                "MEMORY REGION {:<10}: {}..{} ({:<10}) {:?}",
                idx,
                region.range.start,
                region.range.end,
                HumanBytes::from(size),
                region.kind,
            )?;
        }

        Ok(())
    }
}

#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct TlsTemplate {
    /// The offset into the in-memory kernel image where TLS template lives
    pub image_offset: usize,
    /// The size of the TLS segment in memory
    pub mem_size: usize,
    /// The size of the TLS segment in the elf file.
    /// If the TLS segment contains zero-initialized data (tbss) then this size will be smaller than
    /// `mem_size`
    pub file_size: usize,
    pub align: usize,
}
