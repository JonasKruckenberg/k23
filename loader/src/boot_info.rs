use crate::vm::KernelAddressSpace;
use core::alloc::Layout;
use core::ops::Range;
use loader_api::{BootInfo, MemoryRegion, MemoryRegionKind};
use mmu::frame_alloc::{BootstrapAllocator, FrameAllocator};
use mmu::{arch, Error, PhysicalAddress, VirtualAddress};

pub fn init_boot_info(
    mut frame_alloc: BootstrapAllocator,
    boot_hart: usize,
    hart_mask: usize,
    kernel_aspace: &KernelAddressSpace,
    physical_memory_offset: VirtualAddress,
    physical_memory_map: Range<VirtualAddress>,
    fdt_phys: Range<PhysicalAddress>,
    loader_phys: Range<PhysicalAddress>,
    kernel_phys: Range<PhysicalAddress>,
    boot_ticks: u64,
) -> crate::Result<*mut BootInfo> {
    let frame = frame_alloc
        .allocate_contiguous_zeroed(
            Layout::from_size_align(arch::PAGE_SIZE, arch::PAGE_SIZE).unwrap(),
        )
        .ok_or(Error::OutOfMemory)?;
    let page = VirtualAddress::from_phys(frame, physical_memory_offset).unwrap();

    let (memory_regions, memory_regions_len) =
        init_boot_info_memory_regions(page, frame_alloc, fdt_phys, loader_phys.clone());

    let boot_info = page.as_mut_ptr().cast::<BootInfo>();
    unsafe {
        boot_info.write(BootInfo::new(
            boot_hart,
            hart_mask,
            physical_memory_offset,
            physical_memory_map,
            kernel_aspace.kernel_virt.clone(),
            memory_regions,
            memory_regions_len,
            kernel_aspace
                .maybe_tls_allocation
                .as_ref()
                .map(|a| a.tls_template.clone()),
            {
                VirtualAddress::new(loader_phys.start.get()).unwrap()
                    ..VirtualAddress::new(loader_phys.end.get()).unwrap()
            },
            kernel_aspace.heap_virt.clone(),
            kernel_aspace.stacks_virt.clone(),
            kernel_aspace
                .maybe_tls_allocation
                .as_ref()
                .map(|tls| tls.total_region().clone()),
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

    for used_region in frame_alloc.used_regions() {
        push_region(MemoryRegion {
            range: used_region,
            kind: MemoryRegionKind::Loader,
        });
    }

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

    push_region(MemoryRegion {
        range: fdt_phys,
        kind: MemoryRegionKind::FDT,
    });

    (base_ptr, memory_regions_len)
}
