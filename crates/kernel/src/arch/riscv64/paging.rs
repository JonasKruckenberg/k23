use crate::arch::riscv64::MemoryMode;
use crate::arch::{HEAP_BASE, HEAP_SIZE, STACK_SIZE_PAGES};
use crate::board_info::BoardInfo;
use crate::sync::Mutex;
use core::ops::Range;
use core::ptr::addr_of;
use kmem::{Arch, Flush, FrameAllocator, Mapper, PageFlags, PhysicalAddress};
use riscv::register::satp;
use riscv::register::satp::Mode;

static KERNEL_MAPPER: Mutex<Option<Mapper<MemoryMode>>> = Mutex::new(None);

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
    let regions = [stack_region.end..board_info.memory.end];

    log::trace!("physical memory regions {regions:?}");

    // Step 2: initialize allocator
    let frame_alloc = unsafe { FrameAllocator::new(&regions) };

    // Step 4: create mapper
    let mut mapper = Mapper::new(0, frame_alloc)?;

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

    let mmio_range = align_range::<MemoryMode>(board_info.serial.mmio_regs.clone());
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

    for i in 0..HEAP_SIZE / MemoryMode::PAGE_SIZE {
        let virt = HEAP_BASE.add(i * MemoryMode::PAGE_SIZE);
        let frame = mapper.allocator_mut().allocate_frame()?;

        mapper.map_with_flush(virt, frame, PageFlags::READ | PageFlags::WRITE, &mut flush)?;
    }

    unsafe {
        let ppn = mapper.root_table().address().as_raw() >> 12;
        satp::set(Mode::Sv39, mapper.address_space(), ppn);

        log::trace!("flushing address translation changes: {flush:?}");

        flush.flush()?;
    }

    log::trace!("configuring global kernel mapper...");
    KERNEL_MAPPER.lock().replace(mapper);

    // log::debug!("testing heap mapping...");
    // unsafe {
    //     let slice = core::slice::from_raw_parts_mut(HEAP_BASE.as_raw() as *mut u8, HEAP_SIZE);
    //     slice.fill(0);
    // }
    // log::debug!("testing heap success...");

    Ok(())
}

fn align_range<A: Arch>(range: Range<PhysicalAddress>) -> Range<PhysicalAddress> {
    let start = range.start.as_raw() & !(A::PAGE_SIZE - 1);
    let end = (range.end.as_raw() + A::PAGE_SIZE - 1) & !(A::PAGE_SIZE - 1);

    unsafe { PhysicalAddress::new(start)..PhysicalAddress::new(end) }
}
