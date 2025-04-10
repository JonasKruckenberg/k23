// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ops::{Deref, DerefMut};
use core::range::Range;
use core::{fmt, slice};

#[derive(Debug)]
#[non_exhaustive]
pub struct BootInfo {
    pub cpu_mask: usize,
    /// A map of the physical memory regions of the underlying machine.
    ///
    /// The loader parses this information from the firmware and also reports regions used
    /// during initial mapping. Regions marked as usable can be freely
    /// used by the kernel.
    ///
    /// Note: Memory regions are *guaranteed* to not overlap and be sorted by their start address.
    /// But they might not be optimally packed, i.e. adjacent regions that could be merged are not.
    pub memory_regions: MemoryRegions,
    /// Physical addresses can be converted to virtual addresses by adding this offset to them.
    ///
    /// The mapping of the physical memory allows to access arbitrary physical frames. Accessing
    /// frames that are also mapped at other virtual addresses can easily break memory safety and
    /// cause undefined behavior. Only frames reported as `USABLE` by the memory map in the `BootInfo`
    /// can be safely accessed.
    pub physical_address_offset: usize, // VirtualAddress
    pub physical_memory_map: Range<usize>, // VirtualAddress
    /// The thread local storage (TLS) template of the kernel executable, if present.
    pub tls_template: Option<TlsTemplate>,
    /// Virtual address of the loaded kernel image.
    pub kernel_virt: Range<usize>, // VirtualAddress
    /// Physical memory region where the kernel ELF file resides.
    ///
    /// This field can be used by the kernel to perform introspection of its own ELF file.
    pub kernel_phys: Range<usize>, // PhysicalAddress

    pub rng_seed: [u8; 32],
}
unsafe impl Send for BootInfo {}
unsafe impl Sync for BootInfo {}

impl BootInfo {
    /// Create a new boot info structure with the given memory map.
    ///
    /// The other fields are initialized with default values.
    pub fn new(memory_regions: MemoryRegions) -> Self {
        Self {
            memory_regions,
            cpu_mask: 0,
            physical_address_offset: Default::default(),
            physical_memory_map: Default::default(),
            tls_template: None,
            kernel_virt: Default::default(),
            kernel_phys: Default::default(),
            rng_seed: [0; 32],
        }
    }
}

/// FFI-safe slice of [`MemoryRegion`] structs, semantically equivalent to
/// `&'static mut [MemoryRegion]`.
///
/// This type implements the [`Deref`][core::ops::Deref] and [`DerefMut`][core::ops::DerefMut]
/// traits, so it can be used like a `&mut [MemoryRegion]` slice. It also implements [`From`]
/// and [`Into`] for easy conversions from and to `&'static mut [MemoryRegion]`.
#[derive(Debug)]
#[repr(C)]
pub struct MemoryRegions {
    pub(crate) ptr: *mut MemoryRegion,
    pub(crate) len: usize,
}

impl Deref for MemoryRegions {
    type Target = [MemoryRegion];

    fn deref(&self) -> &Self::Target {
        unsafe { slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl DerefMut for MemoryRegions {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl From<&'static mut [MemoryRegion]> for MemoryRegions {
    fn from(regions: &'static mut [MemoryRegion]) -> Self {
        MemoryRegions {
            ptr: regions.as_mut_ptr(),
            len: regions.len(),
        }
    }
}

impl From<MemoryRegions> for &'static mut [MemoryRegion] {
    fn from(regions: MemoryRegions) -> &'static mut [MemoryRegion] {
        unsafe { slice::from_raw_parts_mut(regions.ptr, regions.len) }
    }
}

/// Represent a physical memory region.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[repr(C)]
pub struct MemoryRegion {
    /// The physical start address region.
    pub range: Range<usize>, // PhysicalAddress
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
    /// Unused conventional memory, can be used by the kernel.
    Usable,
    /// Memory mappings created by the loader, including the page table and boot info mappings.
    ///
    /// This memory should _not_ be used by the kernel.
    Loader,
    /// The memory region containing the flattened device tree (FDT).
    FDT,
}

impl MemoryRegionKind {
    pub fn is_usable(&self) -> bool {
        matches!(self, MemoryRegionKind::Usable)
    }
}

impl fmt::Display for BootInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{:<23} : {:#x}",
            "PHYSICAL ADDRESS OFFSET", self.physical_address_offset
        )?;
        writeln!(
            f,
            "{:<23} : {:#x}..{:#x}",
            "PHYSICAL MEMORY MAP", self.physical_memory_map.start, self.physical_memory_map.end
        )?;
        writeln!(
            f,
            "{:<23} : {:#x}..{:#x}",
            "KERNEL VIRT", self.kernel_virt.start, self.kernel_virt.end
        )?;
        writeln!(
            f,
            "{:<23} : {:#x}..{:#x}",
            "KERNEL PHYS", self.kernel_phys.start, self.kernel_phys.end
        )?;
        if let Some(tls) = self.tls_template.as_ref() {
            writeln!(
                f,
                "{:<23} : .tdata: {:#x}..{:#x}, .tbss: {:#x}..{:#x}",
                "TLS TEMPLATE",
                tls.start_addr,
                tls.start_addr.checked_add(tls.file_size).unwrap(),
                tls.start_addr.checked_add(tls.file_size).unwrap(),
                tls.start_addr
                    .checked_add(tls.file_size + tls.mem_size)
                    .unwrap()
            )?;
        } else {
            writeln!(f, "{:<23} : None", "TLS TEMPLATE")?;
            for (idx, region) in self.memory_regions.iter().enumerate() {
                writeln!(
                    f,
                    "MEMORY REGION {:<10}: {:#x}..{:#x} {:?}",
                    idx, region.range.start, region.range.end, region.kind,
                )?;
            }
        }

        Ok(())
    }
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct TlsTemplate {
    /// The address of TLS template
    pub start_addr: usize, // VirtualAddress
    /// The size of the TLS segment in memory
    pub mem_size: usize,
    /// The size of the TLS segment in the elf file.
    /// If the TLS segment contains zero-initialized data (tbss) then this size will be smaller than
    /// `mem_size`
    pub file_size: usize,
    pub align: usize,
}
