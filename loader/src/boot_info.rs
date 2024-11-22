use crate::kernel::Kernel;
use crate::vm::KernelAddressSpace;
use crate::{BumpAllocator, FrameAllocator, PhysicalAddress};
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::slice;
use loader_api::{BootInfo, MemoryRegion, MemoryRegionKind};

pub fn init_boot_info<A>(
    frame_alloc: &mut BumpAllocator<A>,
    boot_hart: usize,
    kernel_aspace: &KernelAddressSpace,
    kernel: &Kernel,
    fdt: PhysicalAddress,
) -> crate::Result<PhysicalAddress>
where
    A: pmm::Arch,
{
    let frame = frame_alloc.allocate_frame()?;

    let memory_regions = init_boot_info_memory_regions(frame_alloc, frame);

    let kernel_phys = {
        let r = kernel.elf_file.input.as_ptr_range();

        PhysicalAddress::new(r.start as usize)..PhysicalAddress::new(r.end as usize)
    };

    todo!();
    // unsafe {
    //     ptr::write(
    //         frame.as_raw() as *mut BootInfo,
    //         BootInfo {
    //             boot_hart,
    //             memory_regions,
    //             tls_template: None,
    //             physmem: kernel_aspace.physical_memory_offset(),
    //             fdt,
    //             loader_region: kernel_aspace.loader_phys().clone(),
    //             kernel_virt: kernel_aspace.kernel_virt(),
    //             kernel_phys,
    //         },
    //     );
    // }

    Ok(frame)
}

fn init_boot_info_memory_regions<A>(
    frame_alloc: &BumpAllocator<A>,
    frame: PhysicalAddress,
) -> NonNull<[MemoryRegion]>
where
    A: pmm::Arch,
{
    // first we need to calculate total slice of regions we could fit in the frame
    let raw_regions = {
        let offset = size_of::<BootInfo>();

        let base_ptr = frame.add(offset).as_raw() as *mut MaybeUninit<MemoryRegion>;
        let num_regions = (A::PAGE_SIZE - offset).div_floor(size_of::<MemoryRegion>());

        unsafe { slice::from_raw_parts_mut(base_ptr, num_regions) }
    };

    let mut next_region = 0;
    let mut push_region = |region: MemoryRegion| {
        raw_regions[next_region].write(region);
        next_region += 1;
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

    unsafe {
        NonNull::from(MaybeUninit::slice_assume_init_mut(
            &mut raw_regions[0..next_region],
        ))
    }
}
