use crate::kconfig;
use arrayvec::ArrayVec;
use core::mem::MaybeUninit;
use kmm::{BitMapAllocator, BumpAllocator, FrameAllocator};
use loader_api::{BootInfo, MemoryRegionKind};
use sync::Mutex;

static FRAME_ALLOC: Mutex<MaybeUninit<BitMapAllocator<kconfig::MEMORY_MODE>>> =
    Mutex::new(MaybeUninit::uninit());

pub fn init<F, R>(boot_info: &BootInfo, f: F) -> R
where
    F: FnOnce(&mut BitMapAllocator<kconfig::MEMORY_MODE>) -> R,
{
    let mut memories = ArrayVec::<_, 16>::new();

    for region in boot_info.memory_regions.iter() {
        if region.kind == MemoryRegionKind::Usable {
            memories.push(region.range.clone());
        }
    }

    let bump_alloc = unsafe { BumpAllocator::new(&memories) };
    let mut alloc = BitMapAllocator::new(bump_alloc).unwrap();
    let r = f(&mut alloc);

    FRAME_ALLOC.lock().write(alloc);

    r
}

pub fn with_frame_alloc<R, F>(f: F) -> R
where
    F: FnOnce(&mut dyn FrameAllocator<kconfig::MEMORY_MODE>) -> R,
{
    let mut alloc = FRAME_ALLOC.lock();
    f(unsafe { alloc.assume_init_mut() })
}
