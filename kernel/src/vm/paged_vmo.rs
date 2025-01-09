use crate::vm::frame_alloc::{Frame, FrameList};
use crate::vm::{frame_alloc, THE_ZERO_FRAME};
use sync::Mutex;

#[derive(Debug)]
pub struct PagedVmo {
    pages: Mutex<FrameList>,
}

impl PagedVmo {
    pub fn require_frame(&self, offset: usize, write: bool) -> crate::Result<Frame> {
        if write {
            self.require_owned_frame(offset)
        } else {
            self.require_read_frame(offset)
        }
    }

    fn require_owned_frame(&self, offset: usize) -> crate::Result<Frame> {
        let mut pages = self.pages.lock();

        if let Some(frame) = pages.get(offset) {
            if frame.is_unique() {
                // we're the sole owner of the frame, so we're good to just return it
                Ok(frame.clone())
            } else if Frame::ptr_eq(frame, &THE_ZERO_FRAME) {
                // the frame is the shared zero frame
                let new_frame = frame_alloc::alloc_one_zeroed()?;
                let old_frame = pages.replace(offset, new_frame.clone());
                log::trace!("old frame {old_frame:?}??");
                Ok(new_frame)
            } else {
                todo!()
            }
        } else {
            todo!("TODO request bytes from source (later when we actually have sources)");
        }

        //     if Frame::ptr_eq(frame, &THE_ZERO_FRAME) {
        //         // Clone the zero frame
        //         let new_frame = frame_alloc::alloc_one_zeroed()?;
        //         let old_frame = pages.replace(offset, new_frame.clone());
        //         log::trace!("old frame {old_frame:?}??");
        //
        //         Ok(new_frame)
        //     } else {
        //         // -> IF FRAME IS OWNED BY VMO (how do we figure out the frame owner?)
        //         //     -> Return frame
        //         // -> IF DIFFERENT OWNER
        //         //     -> do copy on write
        //         //         -> allocate new frame
        //         //         -> copy from frame to new frame
        //         //         -> replace frame
        //         //         -> drop old frame clone (should free if refcount == 1)
        //         //         -> return new frame
        //         todo!()
        //     }
    }

    fn require_read_frame(&self, offset: usize) -> crate::Result<Frame> {
        let pages = self.pages.lock();

        if let Some(frame) = pages.get(offset) {
            log::trace!("require_read_page() frame exists for offset {frame:?}");

            // TODO zirkon clones the zero page here but idk if that necessary
            Ok(frame.clone())
        } else {
            todo!("TODO request bytes from source (later when we actually have sources)");
        }
    }
}

impl FromIterator<Frame> for PagedVmo {
    fn from_iter<T: IntoIterator<Item = Frame>>(iter: T) -> Self {
        Self {
            pages: Mutex::new(FrameList::from_iter(iter)),
        }
    }
}
