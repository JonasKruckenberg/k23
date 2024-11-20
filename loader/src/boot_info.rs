use crate::kconfig;
use crate::kernel::Kernel;
use crate::paging::PageTableResult;
use core::mem::MaybeUninit;
use core::ops::Div;
use core::{ptr, slice};
use kmm::{BumpAllocator, FrameAllocator, PhysicalAddress, VirtualAddress};
use loader_api::{BootInfo, MemoryRegion, MemoryRegionKind};

/// Initialize the `BootInfo` struct in memory that we then pass to the kernel.
pub fn init_boot_info(
    alloc: &mut BumpAllocator<kconfig::MEMORY_MODE>,
    boot_hart: usize,
    page_table_result: &PageTableResult,
    fdt_offset: VirtualAddress,
    kernel: &Kernel,
    physical_memory_offset: VirtualAddress,
) -> crate::Result<&'static BootInfo> {
    let frame = alloc.allocate_frame()?;

    let memory_regions = init_boot_info_memory_regions(alloc, frame);

    // memory_regions: &'static mut [MemoryRegion] is a reference to physical memory, but going forward
    // we need it to be a reference to virtual memory.
    let memory_regions = unsafe { phys_to_virt_mut(physical_memory_offset, memory_regions) };

    let boot_info = unsafe { &mut *(frame.as_raw() as *mut MaybeUninit<BootInfo>) };
    let boot_info = boot_info.write(BootInfo::new(
        boot_hart,
        physical_memory_offset,
        page_table_result.kernel_image_offset,
        memory_regions,
        page_table_result
            .maybe_tls_allocation
            .as_ref()
            .map(|a| a.tls_template.clone()),
        fdt_offset,
        page_table_result.loader_region.clone(),
        {
            let r = kernel.elf_file.data().as_ptr_range();

            PhysicalAddress::new(r.start as usize)..PhysicalAddress::new(r.end as usize)
        },
        page_table_result.heap_virt.clone(),
    ));

    // lastly, do the physical ptr -> virtual ptr translation
    Ok(unsafe { phys_to_virt_ref(physical_memory_offset, boot_info) })
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

/// Convert a reference in physical memory to a reference in virtual memory.
///
/// Note that `wrapping_byte_add` here is *crucial* since we're intentionally wrapping around
/// (from "positive" physmem addresses to "negavtive" kernelspace virtmem addresses). Previously,
/// we relied on the UB of LLVM to do the correct wrapping thing which obviously isn't great.
///
/// # Safety
///
/// well... nothing of this is safe, we're just wholesale making up new references. And references
/// we're actually not even allowed to touch until the MMU is turned on... Maybe we should change the
/// API to use only raw pointers instead...
unsafe fn phys_to_virt_ref<T: ?Sized>(physmem_off: VirtualAddress, phys: &T) -> &T {
    let ptr = (phys as *const T).wrapping_byte_add(physmem_off.as_raw());

    &*ptr
}

/// Convert a mutable reference in physical memory to a reference in virtual memory.
///
/// # Safety
///
/// The same safety rules as [phys_to_virt_ref] apply.
unsafe fn phys_to_virt_mut<T: ?Sized>(physmem_off: VirtualAddress, phys: &mut T) -> &mut T {
    let ptr = ptr::from_mut(phys).wrapping_byte_add(physmem_off.as_raw());

    &mut *ptr
}
