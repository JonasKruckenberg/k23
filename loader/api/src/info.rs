use core::fmt;
use core::fmt::Formatter;
use core::range::Range;
use core::slice;
use mmu::{PhysicalAddress, VirtualAddress};

#[derive(Debug)]
#[repr(C)]
#[non_exhaustive]
pub struct BootInfo {
    /// The hart that booted the machine, for debugging purposes
    pub boot_hart: usize,
    /// A map of the physical memory regions of the underlying machine.
    ///
    /// The bootloader queries this information from the BIOS/UEFI firmware and translates this
    /// information to Rust types. It also marks any memory regions that the bootloader uses in
    /// the memory map before passing it to the kernel. Regions marked as usable can be freely
    /// used by the kernel.
    ///
    /// Note: Memory regions are *guaranteed* to not overlap and be sorted by their start address.
    /// But they might not be optimally packed, i.e. adjacent regions that could be merged are not.
    pub memory_regions: *const MemoryRegion,
    pub memory_regions_len: usize,

    /// The thread local storage (TLS) template of the kernel executable, if present.
    pub tls_template: Option<TlsTemplate>,

    /// Physical addresses can be converted to virtual addresses by adding this offset to them.
    ///
    /// The mapping of the physical memory allows to access arbitrary physical frames. Accessing
    /// frames that are also mapped at other virtual addresses can easily break memory safety and
    /// cause undefined behavior. Only frames reported as `USABLE` by the memory map in the `BootInfo`
    /// can be safely accessed.
    pub physical_address_offset: VirtualAddress,
    pub physical_memory_map: Range<VirtualAddress>,
    /// Virtual memory region occupied by the loader.
    ///
    /// This region is identity-mapped contains the loader executable.
    ///
    /// This is necessary for Risc-V since there is no way for an S-mode loader to atomically
    /// enable paging and jump. The loader must therefore identity-map itself, enable paging and
    /// then jump to the kernel.
    ///
    /// The kernel should use this information to unmap the loader region after taking control.
    pub loader_region: Range<VirtualAddress>,
    /// Virtual address of the loaded kernel image.
    pub kernel_virt: Range<VirtualAddress>,
    /// Physical memory region where the kernel ELF file resides.
    ///
    /// This field can be used by the kernel to perform introspection of its own ELF file.
    pub kernel_elf: Range<PhysicalAddress>,
    pub boot_ticks: u64,
}

unsafe impl Send for BootInfo {}
unsafe impl Sync for BootInfo {}

impl BootInfo {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        boot_hart: usize,
        physical_memory_offset: VirtualAddress,
        physical_memory_map: Range<VirtualAddress>,
        kernel_virt: Range<VirtualAddress>,
        memory_regions: *const MemoryRegion,
        memory_regions_len: usize,
        tls_template: Option<TlsTemplate>,
        loader_region: Range<VirtualAddress>,
        kernel_elf: Range<PhysicalAddress>,
        boot_ticks: u64,
    ) -> Self {
        Self {
            boot_hart,
            physical_address_offset: physical_memory_offset,
            physical_memory_map,
            memory_regions,
            memory_regions_len,
            tls_template,
            kernel_virt,
            loader_region,
            kernel_elf,
            boot_ticks,
        }
    }

    pub fn memory_regions(&self) -> &[MemoryRegion] {
        unsafe { slice::from_raw_parts(self.memory_regions, self.memory_regions_len) }
    }
}

impl fmt::Display for BootInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "{:<23} : {}", "BOOT HART", self.boot_hart)?;
        writeln!(
            f,
            "{:<23} : {}",
            "PHYSICAL MEMORY OFFSET", self.physical_address_offset
        )?;
        writeln!(
            f,
            "{:<23} : {}..{}",
            "KERNEL VIRT", self.kernel_virt.start, self.kernel_virt.end
        )?;
        writeln!(
            f,
            "{:<23} : {}..{}",
            "KERNEL PHYS", self.kernel_elf.start, self.kernel_elf.end
        )?;
        writeln!(
            f,
            "{:<23} : {}..{}",
            "LOADER REGION", self.loader_region.start, self.loader_region.end
        )?;
        if let Some(tls) = self.tls_template.as_ref() {
            writeln!(
                f,
                "{:<23} : .tdata: {}..{}, .tbss: {}..{}",
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
        }
        writeln!(f, "{:<23} : {}", "BOOT TICKS", self.boot_ticks)?;
        for (idx, r) in self.memory_regions().iter().enumerate() {
            writeln!(
                f,
                "MEMORY REGION {:<10}: {}..{} {:?}",
                idx, r.range.start, r.range.end, r.kind,
            )?;
        }

        Ok(())
    }
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
}

/// Represent a physical memory region.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct MemoryRegion {
    /// The physical region.
    pub range: Range<PhysicalAddress>,
    /// The memory type of the memory region.
    ///
    /// Only [`Usable`][MemoryRegionKind::Usable] regions can be freely used.
    pub kind: MemoryRegionKind,
}

/// Represents the different types of memory.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
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
