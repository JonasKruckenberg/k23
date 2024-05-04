use crate::kconfig;
use core::mem::MaybeUninit;
use core::ops::Range;
use sync::Mutex;
use vmm::{BitMapAllocator, BumpAllocator, FrameAllocator, PhysicalAddress};

static FRAME_ALLOC: Mutex<MaybeUninit<BitMapAllocator<kconfig::MEMORY_MODE>>> =
    Mutex::new(MaybeUninit::uninit());

pub fn init<F, R>(memories: &[Range<PhysicalAddress>], alloc_offset: usize, f: F) -> R
where
    F: FnOnce(&mut BitMapAllocator<kconfig::MEMORY_MODE>) -> R,
{
    let bump_alloc = unsafe { BumpAllocator::new(memories, alloc_offset) };
    let mut alloc = BitMapAllocator::new(bump_alloc).unwrap();
    let r = f(&mut alloc);

    FRAME_ALLOC.lock().write(alloc);

    r
}

pub fn with_frame_alloc<R, F>(f: F) -> R
where
    F: FnOnce(&mut dyn FrameAllocator) -> R,
{
    let mut alloc = FRAME_ALLOC.lock();
    f(unsafe { alloc.assume_init_mut() })
}

// pub fn with_mapper<R, F>(asid: usize, mut f: F) -> Result<R, vmm::Error>
// where
//     F: FnMut(
//         Mapper<kconfig::MEMORY_MODE>,
//         &mut Flush<kconfig::MEMORY_MODE>,
//     ) -> Result<R, vmm::Error>,
// {
//     with_frame_alloc(|alloc| {
//         let mapper = Mapper::from_active(asid, alloc);
//
//         let mut flush = Flush::empty(asid);
//
//         let r = f(mapper, &mut flush)?;
//
//         flush.flush()?;
//
//         Ok(r)
//     })
// }
