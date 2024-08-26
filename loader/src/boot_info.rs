use crate::kconfig;
use crate::paging::PageTableResult;
use core::mem::MaybeUninit;
use core::ops::Div;
use core::slice;
use kmm::{BumpAllocator, FrameAllocator, PhysicalAddress, VirtualAddress};
use loader_api::{BootInfo, MemoryRegion, MemoryRegionKind};

pub fn init_boot_info(
    alloc: &mut BumpAllocator<kconfig::MEMORY_MODE>,
    boot_hart: usize,
    page_table_result: &PageTableResult,
    fdt_virt: VirtualAddress,
    physmem_off: VirtualAddress,
) -> crate::Result<&'static BootInfo> {
    let frame = alloc.allocate_frame()?;

    let memory_regions = init_boot_info_memory_regions(alloc, frame);

    // memory_regions: &'static mut [MemoryRegion] is a reference to physical memory, but going forward
    // we need it to be a reference to virtual memory.
    let memory_regions = unsafe {
        let ptr = memory_regions.as_mut_ptr().byte_add(physmem_off.as_raw());
        slice::from_raw_parts_mut(ptr, memory_regions.len())
    };

    let boot_info = unsafe { &mut *(frame.as_raw() as *mut MaybeUninit<BootInfo>) };
    let boot_info = boot_info.write(BootInfo::new(
        boot_hart,
        physmem_off,
        memory_regions,
        page_table_result
            .maybe_tls_allocation
            .as_ref()
            .map(|a| a.tls_template.clone()),
        fdt_virt,
        page_table_result.loader_virt.clone(),
        page_table_result.free_range_virt.clone(),
    ));

    // lastly, do the physical ptr -> virtual ptr translation
    Ok(unsafe { phys_to_virt_ref(physmem_off, boot_info) })
}

fn init_boot_info_memory_regions(
    alloc: &BumpAllocator<kconfig::MEMORY_MODE>,
    frame: PhysicalAddress,
) -> &'static mut [MemoryRegion] {
    // first we need to calculate total slice of regions we could fit in the frame
    let raw_regions = {
        let offset = size_of::<BootInfo>();

        let base_ptr = frame.add(offset).as_raw() as *mut MaybeUninit<MemoryRegion>;
        let num_regions = (kconfig::PAGE_SIZE - offset).div(size_of::<MemoryRegion>());

        unsafe { slice::from_raw_parts_mut(base_ptr, num_regions) }
    };

    let mut next_region = 0;
    let mut push_region = |region: MemoryRegion| {
        raw_regions[next_region].write(region);
        next_region += 1;
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

    unsafe { MaybeUninit::slice_assume_init_mut(&mut raw_regions[0..next_region]) }
}

unsafe fn phys_to_virt_ref<T>(physmem_off: VirtualAddress, phys: &T) -> &T {
    let ptr = (phys as *const T).byte_add(physmem_off.as_raw());

    &*ptr
}
