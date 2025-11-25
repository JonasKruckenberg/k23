// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::fmt;
use core::ops::Range;
use core::ptr::NonNull;

use arrayvec::ArrayVec;
use kmem_core::{AddressSpace, AllocError, Arch, FrameAllocator, PhysicalAddress, VirtualAddress};

pub const MAX_REGIONS: usize = 32;

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
    pub memory_regions: ArrayVec<MemoryRegion, MAX_REGIONS>,

    pub address_space: AddressSpace,

    pub physical_memory_map: Range<VirtualAddress>,

    // pub physical_memory_map: Range<VirtualAddress>,
    /// The thread local storage (TLS) template of the kernel executable, if present.
    pub tls_template: Option<TlsTemplate>,
    /// Virtual address of the loaded kernel image.
    pub kernel_virt: Range<VirtualAddress>,
    /// Physical memory region where the kernel ELF file resides.
    ///
    /// This field can be used by the kernel to perform introspection of its own ELF file.
    pub kernel_phys: Range<PhysicalAddress>,

    pub rng_seed: [u8; 32],
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct TlsTemplate {
    /// The address of TLS template
    pub start_addr: VirtualAddress,
    /// The size of the TLS segment in memory
    pub mem_size: usize,
    /// The size of the TLS segment in the elf file.
    /// If the TLS segment contains zero-initialized data (tbss) then this size will be smaller than
    /// `mem_size`
    pub file_size: usize,
    pub align: usize,
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
        writeln!(f, "{:<23} : {:b}", "CPU MASK", self.cpu_mask)?;
        writeln!(f, "{:<23} : {:x?}", "RNG SEED", self.rng_seed)?;
        writeln!(f, "{:<23} : {:?}", "ADDRESS SPACE", self.address_space)?;
        writeln!(
            f,
            "{:<23} : {}..{}",
            "PHYSICAL MEMORY MAP", self.physical_memory_map.start, self.physical_memory_map.end
        )?;
        writeln!(
            f,
            "{:<23} : {}..{}",
            "KERNEL VIRT", self.kernel_virt.start, self.kernel_virt.end
        )?;
        writeln!(
            f,
            "{:<23} : {}..{}",
            "KERNEL PHYS", self.kernel_phys.start, self.kernel_phys.end
        )?;
        if let Some(tls) = self.tls_template.as_ref() {
            writeln!(
                f,
                "{:<23} : .tdata: {:?}..{:?}, .tbss: {:?}..{:?}",
                "TLS TEMPLATE",
                tls.start_addr,
                tls.start_addr.add(tls.file_size),
                tls.start_addr.add(tls.file_size),
                tls.start_addr.add(tls.file_size + tls.mem_size)
            )?;
        } else {
            writeln!(f, "{:<23} : None", "TLS TEMPLATE")?;
        }

        for (idx, region) in self.memory_regions.iter().enumerate() {
            writeln!(
                f,
                "MEMORY REGION {:<10}: {}..{} {:?}",
                idx, region.range.start, region.range.end, region.kind,
            )?;
        }

        Ok(())
    }
}

pub struct BootInfoBuilder {
    under_construction: BootInfo,
}

impl BootInfoBuilder {
    pub fn new(address_space: AddressSpace) -> Self {
        Self {
            under_construction: BootInfo {
                address_space,
                cpu_mask: 0,
                memory_regions: ArrayVec::new(),
                physical_memory_map: VirtualAddress::MIN..VirtualAddress::MIN,
                tls_template: None,
                kernel_virt: VirtualAddress::MIN..VirtualAddress::MIN,
                kernel_phys: PhysicalAddress::MIN..PhysicalAddress::MIN,
                rng_seed: [0u8; 32],
            },
        }
    }

    pub fn with_cpu_mask(mut self, cpu_mask: usize) -> Self {
        self.under_construction.cpu_mask = cpu_mask;
        self
    }

    pub fn with_memory_region(mut self, memory_region: MemoryRegion) -> Self {
        self.under_construction.memory_regions.push(memory_region);
        self
    }

    pub fn with_memory_regions(
        mut self,
        memory_regions: impl IntoIterator<Item = MemoryRegion>,
    ) -> Self {
        self.under_construction
            .memory_regions
            .extend(memory_regions);
        self
    }

    pub fn with_physical_memory_map(mut self, physical_memory_map: Range<VirtualAddress>) -> Self {
        self.under_construction.physical_memory_map = physical_memory_map;
        self
    }

    pub fn with_tls_template(mut self, tls_template: TlsTemplate) -> Self {
        self.under_construction.tls_template = Some(tls_template);
        self
    }

    pub fn with_kernel_virt(mut self, kernel_virt: Range<VirtualAddress>) -> Self {
        self.under_construction.kernel_virt = kernel_virt;
        self
    }

    pub fn with_kernel_phys(mut self, kernel_phys: Range<PhysicalAddress>) -> Self {
        self.under_construction.kernel_phys = kernel_phys;
        self
    }

    pub fn with_rng_seed(mut self, rng_seed: [u8; 32]) -> Self {
        self.under_construction.rng_seed = rng_seed;
        self
    }

    pub fn finish(self) -> BootInfo {
        self.under_construction
    }

    pub fn finish_and_allocate(
        self,
        frame_alloc: impl FrameAllocator,
    ) -> Result<NonNull<BootInfo>, AllocError> {
        let info = self.finish();

        let phys = frame_alloc
            .allocate_contiguous_zeroed(Layout::for_value(&info), info.address_space.arch())?;

        let virt = info.address_space.arch().phys_to_virt(phys);

        let ptr = unsafe { virt.as_non_null().unwrap_unchecked().cast::<BootInfo>() };

        unsafe { ptr.write(info) };

        Ok(ptr)
    }
}
