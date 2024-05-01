use crate::kconfig;
use core::mem::MaybeUninit;
use core::ops::Range;
use sync::Mutex;
use vmm::{BitMapAllocator, BumpAllocator, Flush, FrameAllocator, Mapper, PhysicalAddress};

static FRAME_ALLOC: Mutex<MaybeUninit<BitMapAllocator<kconfig::MEMORY_MODE>>> =
    Mutex::new(MaybeUninit::uninit());

pub fn init(memories: &[Range<PhysicalAddress>], alloc_offset: usize) {
    let bump_alloc = unsafe { BumpAllocator::new(memories, alloc_offset) };
    let alloc = BitMapAllocator::new(bump_alloc).unwrap();
    alloc.debug_print_table();

    FRAME_ALLOC.lock().write(alloc);
}

pub fn with_frame_alloc<R, F>(f: F) -> R
where
    F: FnOnce(&mut dyn FrameAllocator) -> R,
{
    let mut alloc = FRAME_ALLOC.lock();
    f(unsafe { alloc.assume_init_mut() })
}

pub fn with_mapper<R, F>(asid: usize, mut f: F) -> Result<R, vmm::Error>
where
    F: FnMut(
        Mapper<kconfig::MEMORY_MODE>,
        &mut Flush<kconfig::MEMORY_MODE>,
    ) -> Result<R, vmm::Error>,
{
    with_frame_alloc(|alloc| {
        let mapper = Mapper::from_active(asid, alloc);

        let mut flush = Flush::empty(asid);

        let r = f(mapper, &mut flush)?;

        flush.flush()?;

        Ok(r)
    })
}
