use crate::arch::riscv64::MemoryMode;
use crate::arch::{HEAP_BASE, HEAP_SIZE, STACK_SIZE_PAGES};
use crate::board_info::BoardInfo;
use crate::sync::Mutex;
use core::ops::Range;
use core::ptr::addr_of;
use kmem::{
    AddressRange, Arch, BumpAllocator, Flush, FrameAllocator, Mapper, PageFlags, PhysicalAddress,
};

pub static FRAME_ALLOC: Mutex<Option<FrameAllocator<MemoryMode>>> = Mutex::new(None);

static mut MEMORY_REGIONS: [Range<PhysicalAddress>; 1] =
    unsafe { [PhysicalAddress::new(0)..PhysicalAddress::new(0)] };

pub fn init(board_info: &BoardInfo) -> crate::Result<()> {
    extern "C" {
        static __text_start: u8;
        static __text_end: u8;
        static __stack_start: u8;
        static __rodata_start: u8;
        static __eh_frame_end: u8;
    }

    let stack_start = unsafe { addr_of!(__stack_start) as usize };
    let text_start = unsafe { addr_of!(__text_start) as usize };
    let text_end = unsafe { addr_of!(__text_end) as usize };
    let rodata_start = unsafe { addr_of!(__rodata_start) as usize };
    let eh_frame_end = unsafe { addr_of!(__eh_frame_end) as usize };

    let stack_region = unsafe {
        let start = PhysicalAddress::new(stack_start);

        start..start.add(STACK_SIZE_PAGES * MemoryMode::PAGE_SIZE * board_info.cpus)
    };

    let kernel_executable_region =
        unsafe { PhysicalAddress::new(text_start)..PhysicalAddress::new(text_end) };

    let kernel_read_only_region =
        unsafe { PhysicalAddress::new(rodata_start)..PhysicalAddress::new(eh_frame_end) };

    let kernel_read_write_region =
        unsafe { PhysicalAddress::new(text_end)..PhysicalAddress::new(stack_start) };

    // Step 1: collect physical memory regions
    // these are all the addresses we can use for allocation
    // which should get this info from the DTB ideally
    unsafe {
        MEMORY_REGIONS = [board_info.memory.start..board_info.memory.end];
        log::trace!("physical memory regions {MEMORY_REGIONS:?}");
    }

    // Step 2: initialize allocator
    let bump_alloc: BumpAllocator<MemoryMode> = unsafe {
        BumpAllocator::new(
            &MEMORY_REGIONS,
            stack_region.end.as_raw() - board_info.memory.start.as_raw(),
        )
    };
    let frame_usage = bump_alloc.memory_usage();
    log::info!(
        "Used {}MiB of {}MiB total for kernel disk image & stack region. Available {}MiB {:?}..{:?}",
        frame_usage.used * 4 / 1024,
        frame_usage.total * 4 / 1024,
        (frame_usage.total - frame_usage.used) * 4 / 1024,
        board_info.memory.start,
        stack_region.start
        // stack_region.end.as_raw() - board_info.memory.start.as_raw()
    );

    let mut frame_alloc = FrameAllocator::new(bump_alloc)?;

    // Step 4: create mapper
    let mut mapper = Mapper::new(0, &mut frame_alloc).unwrap();

    let mut flush = Flush::empty(0);

    log::trace!(
        "Identity mapping kernel executable region {:?}",
        kernel_executable_region,
    );
    mapper.identity_map_range_with_flush(
        kernel_executable_region,
        PageFlags::READ | PageFlags::EXECUTE,
        &mut flush,
    )?;

    log::trace!(
        "Identity mapping kernel read only region {:?}",
        kernel_read_only_region,
    );
    mapper.identity_map_range_with_flush(kernel_read_only_region, PageFlags::READ, &mut flush)?;

    log::trace!(
        "Identity mapping kernel read-write section {:?}",
        kernel_read_write_region,
    );
    mapper.identity_map_range_with_flush(
        kernel_read_write_region,
        PageFlags::READ | PageFlags::WRITE,
        &mut flush,
    )?;

    log::trace!("Identity mapping kernel stack region {:?}", stack_region,);
    mapper.identity_map_range_with_flush(
        stack_region.clone(),
        PageFlags::READ | PageFlags::WRITE,
        &mut flush,
    )?;

    let mmio_range = board_info
        .serial
        .mmio_regs
        .clone()
        .align(MemoryMode::PAGE_SIZE);
    log::trace!("Identity mapping mmio region {:?}", mmio_range,);
    mapper.identity_map_range_with_flush(
        mmio_range,
        PageFlags::READ | PageFlags::WRITE,
        &mut flush,
    )?;

    log::trace!(
        "Mapping kernel heap {:?}",
        HEAP_BASE..HEAP_BASE.add(HEAP_SIZE)
    );

    let heap_phys = {
        let base = mapper
            .allocator_mut()
            .allocate_frames(HEAP_SIZE / MemoryMode::PAGE_SIZE)?;
        base..base.add(HEAP_SIZE)
    };
    let heap_virt = HEAP_BASE..HEAP_BASE.add(HEAP_SIZE);

    mapper.map_range_with_flush(
        heap_virt,
        heap_phys,
        PageFlags::READ | PageFlags::WRITE,
        &mut flush,
    )?;

    let frame_usage = mapper.allocator().frame_usage();
    log::info!(
        "Physical memory after mapping: Used {}MiB of {}MiB, available {}MiB",
        frame_usage.used * 4 / 1024,
        frame_usage.total * 4 / 1024,
        (frame_usage.total - frame_usage.used) * 4 / 1024
    );

    // Debug: Used 64MiB of 122MiB, available 58MiB
    // Release: Used 64MiB of 125MiB, available 61MiB

    todo!()
    //
    // log::trace!("activating page table...");
    // mapper.activate();
    // log::trace!("flushing address translation changes: {flush:?}");
    // flush.flush()?;
    //
    // log::trace!("initializing global frame allocator...");
    // FRAME_ALLOC.lock().replace(frame_alloc);
    //
    // // log::debug!("testing heap mapping...");
    // // unsafe {
    // //     let slice = core::slice::from_raw_parts_mut(HEAP_BASE.as_raw() as *mut u8, HEAP_SIZE);
    // //     slice.fill(0);
    // // }
    // // log::debug!("testing heap success...");
    //
    // Ok(())
}
