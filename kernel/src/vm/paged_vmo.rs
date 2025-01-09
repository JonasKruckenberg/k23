use crate::vm::frame_alloc::{Frame, FrameList};
use crate::vm::{frame_alloc, THE_ZERO_FRAME};
use mmu::VirtualAddress;

#[derive(Debug)]
pub struct PagedVmo {
    frames: FrameList,
}

impl FromIterator<Frame> for PagedVmo {
    fn from_iter<T: IntoIterator<Item = Frame>>(iter: T) -> Self {
        Self {
            frames: FrameList::from_iter(iter),
        }
    }
}

impl PagedVmo {
    pub fn require_frame(
        &mut self,
        at_offset: usize,
        will_write: bool,
        phys_off: VirtualAddress,
    ) -> crate::Result<&Frame> {
        if will_write {
            self.require_owned_frame(at_offset, phys_off)
        } else {
            self.require_read_frame(at_offset)
        }
    }

    fn require_owned_frame(
        &mut self,
        at_offset: usize,
        phys_off: VirtualAddress,
    ) -> crate::Result<&Frame> {
        if let Some(old_frame) = self.frames.get(at_offset) {
            log::trace!("require_owned_frame for resident frame, allocating new...");

            let mut new_frame = frame_alloc::alloc_one_zeroed()?;

            // If `old_frame` is the zero frame we don't need to copy any data around, it's
            // all zeroes anyway
            if !Frame::ptr_eq(old_frame, &THE_ZERO_FRAME) {
                log::trace!("performing copy-on-write...");
                let src = old_frame.as_slice(phys_off);
                let dst = Frame::get_mut(&mut new_frame)
                    .expect("newly allocated frame should be unqiue")
                    .as_mut_slice(phys_off);

                dst.copy_from_slice(src);
            }

            let new_frame = self.frames.insert(at_offset, new_frame.clone());
            Ok(new_frame)
        } else {
            todo!("TODO request bytes from source (later when we actually have sources)");
        }
    }

    fn require_read_frame(&self, at_offset: usize) -> crate::Result<&Frame> {
        if let Some(frame) = self.frames.get(at_offset) {
            log::warn!("required resident read frame, this should not happen!");
            Ok(frame)
        } else {
            todo!("TODO request bytes from source (later when we actually have sources)");
        }
    }
}
