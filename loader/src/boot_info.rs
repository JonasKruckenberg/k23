use crate::kernel::Kernel;
use crate::vm::KernelAddressSpace;
use core::alloc::Layout;
use core::ops::Range;
use loader_api::{BootInfo, MemoryRegion, MemoryRegionKind};
use pmm::frame_alloc::{BuddyAllocator, FrameAllocator};
use pmm::{arch, Error, PhysicalAddress, VirtualAddress};

pub fn init_boot_info(
    mut frame_alloc: BuddyAllocator,
    boot_hart: usize,
    kernel: &Kernel,
    kernel_aspace: &KernelAddressSpace,
    physical_memory_offset: VirtualAddress,
    fdt_phys: Range<PhysicalAddress>,
    loader_phys: Range<PhysicalAddress>,
) -> crate::Result<*mut BootInfo> {
    let frame = frame_alloc
        .allocate_contiguous_zeroed(
            Layout::from_size_align(arch::PAGE_SIZE, arch::PAGE_SIZE).unwrap(),
        )
        .ok_or(Error::OutOfMemory)?;
    let page = physical_memory_offset.add(frame.as_raw());

    let (memory_regions, memory_regions_len) =
        init_boot_info_memory_regions(page, frame_alloc, fdt_phys);

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
            {
                VirtualAddress::new(loader_phys.start.as_raw())
                    ..VirtualAddress::new(loader_phys.end.as_raw())
            },
            kernel_aspace.heap_virt.clone(),
            {
                let r = kernel.elf_file.input.as_ptr_range();

                PhysicalAddress::new(r.start as usize)..PhysicalAddress::new(r.end as usize)
            },
        ));
    }

    Ok(boot_info)
}

fn init_boot_info_memory_regions(
    page: VirtualAddress,
    frame_alloc: BuddyAllocator,
    fdt_phys: Range<PhysicalAddress>,
) -> (*mut MemoryRegion, usize) {
    let base_ptr = page.add(size_of::<BootInfo>()).as_raw() as *mut MemoryRegion;
    let mut ptr = base_ptr;
    let mut memory_regions_len = 0;
    let max_regions = (arch::PAGE_SIZE - size_of::<BootInfo>()) / size_of::<MemoryRegion>();

    let mut push_region = |region: MemoryRegion| unsafe {
        assert!(memory_regions_len < max_regions);
        ptr.write(region);
        ptr = ptr.add(1);
        memory_regions_len += 1;
    };

    for region in frame_alloc.into_iter() {
        push_region(MemoryRegion {
            range: region,
            kind: MemoryRegionKind::Usable,
        });
    }

    // for used_region in alloc.used_regions() {
    //     push_region(MemoryRegion {
    //         range: used_region,
    //         kind: MemoryRegionKind::Loader,
    //     });
    // }
    //
    // for free_region in alloc.free_regions() {
    //     push_region(MemoryRegion {
    //         range: free_region,
    //         kind: MemoryRegionKind::Usable,
    //     });
    // }

    push_region(MemoryRegion {
        range: fdt_phys,
        kind: MemoryRegionKind::FDT,
    });

    (base_ptr, memory_regions_len)
}
