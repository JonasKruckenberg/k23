use core::ops::Range;
use pmm::{PhysicalAddress, VirtualAddress};

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
    pub memory_regions: *const MemoryRegion,
    pub memory_regions_len: usize,
    /// The thread local storage (TLS) template of the kernel executable, if present.
    ///
    /// Note that the loader will already set up TLS regions for each hart reported as `online`
    /// by the previous stage bootloader, so this field is rarely needed. Only when the kernel
    /// has ways to bring new harts online after booting, this field is useful.
    pub tls_template: Option<TlsTemplate>,
    /// The virtual address at which the mapping of the physical memory starts.
    ///
    /// Physical addresses can be converted to virtual addresses by adding this offset to them.
    ///
    /// The mapping of the physical memory allows to access arbitrary physical frames. Accessing
    /// frames that are also mapped at other virtual addresses can easily break memory safety and
    /// cause undefined behavior. Only frames reported as `USABLE` by the memory map in the `BootInfo`
    /// can be safely accessed.
    pub physical_memory_offset: VirtualAddress,
    /// Virtual address of the flattened device tree.
    pub fdt_offset: VirtualAddress,
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
    /// Virtual memory region reserved for the kernel heap.
    ///
    /// Note that this is **not** mapped, as the kernel should map
    /// this region on-demand.
    pub heap_region: Option<Range<VirtualAddress>>,
    /// Virtual address of the loaded kernel image.
    pub kernel_virt: Range<VirtualAddress>,
    /// Physical memory region where the kernel ELF file resides.
    ///
    /// This field can be used by the kernel to perform introspection of its own ELF file.
    pub kernel_elf: Range<PhysicalAddress>,
}

unsafe impl Send for BootInfo {}
unsafe impl Sync for BootInfo {}

impl BootInfo {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        boot_hart: usize,
        physical_memory_offset: VirtualAddress,
        kernel_virt: Range<VirtualAddress>,
        memory_regions: *const MemoryRegion,
        memory_regions_len: usize,
        tls_template: Option<TlsTemplate>,
        fdt_offset: VirtualAddress,
        loader_region: Range<VirtualAddress>,
        heap_region: Option<Range<VirtualAddress>>,
        kernel_elf: Range<PhysicalAddress>,
    ) -> Self {
        Self {
            boot_hart,
            physical_memory_offset,
            memory_regions,
            memory_regions_len,
            tls_template,
            fdt_offset,
            kernel_virt,
            loader_region,
            heap_region,
            kernel_elf,
        }
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
#[derive(Debug)]
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
    /// Memory mappings created by the bootloader, including the page table and boot info mappings.
    ///
    /// This memory should _not_ be used by the kernel.
    Loader,
}

impl MemoryRegionKind {
    #[must_use]
    pub fn is_usable(&self) -> bool {
        matches!(self, MemoryRegionKind::Usable)
    }
}
