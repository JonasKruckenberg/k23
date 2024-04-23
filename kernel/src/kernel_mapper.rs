use crate::kconfig;
use core::mem::MaybeUninit;
use core::ops::Range;
use sync::Mutex;
use vmm::{BitMapAllocator, BumpAllocator, Flush, Mapper, PhysicalAddress};

static KERNEL_MAPPER: Mutex<
    MaybeUninit<Mapper<kconfig::MEMORY_MODE, BitMapAllocator<kconfig::MEMORY_MODE>>>,
> = Mutex::new(MaybeUninit::uninit());

pub fn init(memories: &[Range<PhysicalAddress>], alloc_offset: usize) {
    let bump_alloc = unsafe { BumpAllocator::new(memories, alloc_offset) };
    let alloc = BitMapAllocator::new(bump_alloc).unwrap();
    alloc.debug_print_table();

    KERNEL_MAPPER.lock().write(Mapper::from_active(0, alloc));
}

pub fn with_kernel_mapper<R, F>(mut f: F) -> Result<R, vmm::Error>
where
    F: FnMut(
        &mut Mapper<kconfig::MEMORY_MODE, BitMapAllocator<kconfig::MEMORY_MODE>>,
        &mut Flush<kconfig::MEMORY_MODE>,
    ) -> Result<R, vmm::Error>,
{
    let mut mapper = KERNEL_MAPPER.lock();
    let mut flush = Flush::empty(0);

    let r = f(unsafe { mapper.assume_init_mut() }, &mut flush)?;

    flush.flush()?;

    Ok(r)
}
