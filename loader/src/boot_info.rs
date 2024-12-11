use crate::kernel::Kernel;
use crate::vm::KernelAddressSpace;
use loader_api::{BootInfo, MemoryRegion, MemoryRegionKind};
use pmm::frame_alloc::{BumpAllocator, FrameAllocator};
use pmm::{PhysicalAddress, VirtualAddress};

pub fn init_boot_info<A>(
    alloc: &mut BumpAllocator,
    boot_hart: usize,
    kernel: &Kernel,
    kernel_aspace: &KernelAddressSpace<A>,
    physical_memory_offset: VirtualAddress,
    fdt_offset: VirtualAddress,
) -> crate::Result<*mut BootInfo> {
    let page =
        physical_memory_offset.add(alloc.allocate_one_zeroed(physical_memory_offset)?.as_raw());

    let (memory_regions, memory_regions_len) = init_boot_info_memory_regions(alloc, page);

    let boot_info = page.as_raw() as *mut BootInfo;
    unsafe {
        boot_info.write(BootInfo::new(
            boot_hart,
            physical_memory_offset,
            kernel_aspace.kernel_virt.clone(),
            memory_regions,
            memory_regions_len,
            kernel_aspace
                .maybe_tls_allocation
                .as_ref()
                .map(|a| a.tls_template.clone()),
            fdt_offset,
            kernel_aspace.loader_region.clone(),
            kernel_aspace.heap_virt.clone(),
            {
                let r = kernel.elf_file.input.as_ptr_range();

                PhysicalAddress::new(r.start as usize)..PhysicalAddress::new(r.end as usize)
            },
        ));
    }

    Ok(boot_info)
}

fn init_boot_info_memory_regions(alloc: &BumpAllocator, page: VirtualAddress) -> (*mut MemoryRegion, usize) {
    let base_ptr = page.add(size_of::<BootInfo>()).as_raw() as *mut MemoryRegion;
    let mut ptr = base_ptr;
    let mut memory_regions_len = 0;

    let mut push_region = |region: MemoryRegion| unsafe {
        ptr.write(region);
        ptr = ptr.add(1);
        memory_regions_len += 1;
    };

    for used_region in alloc.used_regions() {
        push_region(MemoryRegion {
            range: used_region,
            kind: MemoryRegionKind::Loader,
        });
    }

    for free_region in alloc.free_regions() {
        push_region(MemoryRegion {
            range: free_region,
            kind: MemoryRegionKind::Usable,
        });
    }

    (base_ptr, memory_regions_len)
}
