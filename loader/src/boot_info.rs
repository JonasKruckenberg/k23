use crate::error::Error;
use core::alloc::Layout;
use core::range::Range;
use core::slice;
use loader_api::{BootInfo, MemoryRegion, MemoryRegionKind, TlsTemplate};
use mmu::frame_alloc::{BootstrapAllocator, FrameAllocator};
use mmu::{arch, PhysicalAddress, VirtualAddress};

pub fn prepare_boot_info(
    mut frame_alloc: BootstrapAllocator,
    boot_hart: usize,
    physical_memory_offset: VirtualAddress,
    physical_memory_map: Range<VirtualAddress>,
    kernel_virt: Range<VirtualAddress>,
    maybe_tls_template: Option<TlsTemplate>,
    loader_phys: Range<PhysicalAddress>,
    kernel_phys: Range<PhysicalAddress>,
    fdt_phys: Range<PhysicalAddress>,
    boot_ticks: u64,
) -> crate::Result<*mut BootInfo> {
    let frame = frame_alloc
        .allocate_contiguous_zeroed(
            Layout::from_size_align(arch::PAGE_SIZE, arch::PAGE_SIZE).unwrap(),
        )
        .ok_or(Error::NoMemory)?;
    let page = VirtualAddress::from_phys(frame, physical_memory_offset).unwrap();

    let (memory_regions, memory_regions_len) =
        init_boot_info_memory_regions(page, frame_alloc, fdt_phys, loader_phys.clone());

    let boot_info = page.as_mut_ptr().cast::<BootInfo>();
    unsafe {
        boot_info.write(BootInfo::new(
            boot_hart,
            physical_memory_offset,
            physical_memory_map,
            kernel_virt,
            memory_regions,
            memory_regions_len,
            maybe_tls_template,
            Range::from(
                VirtualAddress::new(loader_phys.start.get()).unwrap()
                    ..VirtualAddress::new(loader_phys.end.get()).unwrap(),
            ),
            kernel_phys,
            boot_ticks,
        ));
    }

    Ok(boot_info)
}

fn init_boot_info_memory_regions(
    page: VirtualAddress,
    frame_alloc: BootstrapAllocator,
    fdt_phys: Range<PhysicalAddress>,
    loader_phys: Range<PhysicalAddress>,
) -> (*mut MemoryRegion, usize) {
    let base_ptr = page
        .checked_add(size_of::<BootInfo>())
        .unwrap()
        .as_mut_ptr()
        .cast::<MemoryRegion>();
    let mut ptr = base_ptr;
    let mut memory_regions_len = 0;
    let max_regions = (arch::PAGE_SIZE - size_of::<BootInfo>()) / size_of::<MemoryRegion>();

    let mut push_region = |region: MemoryRegion| unsafe {
        assert!(memory_regions_len < max_regions);
        ptr.write(region);
        ptr = ptr.add(1);
        memory_regions_len += 1;
    };

    // Report the memory we consumed during startup as used.
    for used_region in frame_alloc.used_regions() {
        push_region(MemoryRegion {
            range: used_region,
            kind: MemoryRegionKind::Loader,
        });
    }

    // Report the free regions as usable.
    for free_region in frame_alloc.free_regions() {
        push_region(MemoryRegion {
            range: free_region,
            kind: MemoryRegionKind::Usable,
        });
    }

    // The memory occupied by the loader is not needed once the kernel is running.
    // Mark it as usable.
    push_region(MemoryRegion {
        range: loader_phys,
        kind: MemoryRegionKind::Usable,
    });

    // Report the flattened device tree as a separate region.
    push_region(MemoryRegion {
        range: fdt_phys,
        kind: MemoryRegionKind::FDT,
    });

    // Sort the memory regions by start address, we do this now in the loader
    // because the BootInfo struct will be passed as a read-only static reference to the kernel.
    unsafe {
        slice::from_raw_parts_mut(base_ptr, memory_regions_len)
            .sort_unstable_by_key(|region| region.range.start);
    }

    (base_ptr, memory_regions_len)
}
