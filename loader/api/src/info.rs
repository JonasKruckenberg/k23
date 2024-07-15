use core::ops::Range;
use vmm::{PhysicalAddress, VirtualAddress};

#[derive(Debug)]
#[repr(C)]
#[non_exhaustive]
pub struct BootInfo {
    /// The hart that booted the machine, for debugging purposes
    pub boot_hart: u32,
    /// The virtual address at which the mapping of the physical memory starts.
    ///
    /// Physical addresses can be converted to virtual addresses by adding this offset to them.
    ///
    /// The mapping of the physical memory allows to access arbitrary physical frames. Accessing
    /// frames that are also mapped at other virtual addresses can easily break memory safety and
    /// cause undefined behavior. Only frames reported as `USABLE` by the memory map in the `BootInfo`
    /// can be safely accessed.
    pub physical_memory_offset: VirtualAddress,
    /// A map of the physical memory regions of the underlying machine.
    ///
    /// The bootloader queries this information from the BIOS/UEFI firmware and translates this
    /// information to Rust types. It also marks any memory regions that the bootloader uses in
    /// the memory map before passing it to the kernel. Regions marked as usable can be freely
    /// used by the kernel.
    pub memory_regions: &'static mut [MemoryRegion],
    /// The thread local storage (TLS) template of the kernel executable, if present.
    pub tls_template: Option<vmm::TlsTemplate>,
    /// Address of the flattened device tree
    pub fdt_virt: Option<VirtualAddress>,
    /// The virtual memory occupied by the bootloader.
    pub loader_virt: Option<Range<VirtualAddress>>,
    /// The range of addresses that the kernel can freely allocate from.
    pub free_virt: Range<VirtualAddress>,
}

impl BootInfo {
    /// Create a new boot info structure with the given memory map.
    ///
    /// The other fields are initialized with default values.
    pub fn new(memory_regions: &'static mut [MemoryRegion]) -> Self {
        Self {
            boot_hart: 0,
            memory_regions,
            physical_memory_offset: VirtualAddress::default(),
            tls_template: None,
            fdt_virt: None,
            loader_virt: None,
            free_virt: Range::default(),
        }
    }
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
