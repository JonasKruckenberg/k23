use crate::error::Error;
use crate::vm::frame_alloc;
use crate::vm::frame_alloc::{Frame, FrameList};
use crate::vm::THE_ZERO_FRAME;
use sync::Mutex;

#[derive(Debug)]
pub struct PagedVmo {
    pages: Mutex<FrameList>,
}

impl PagedVmo {
    pub fn require_owned_page(&self, offset: usize) -> crate::Result<Frame> {
        let mut pages = self.pages.lock();

        if let Some(frame) = pages.get(offset) {
            log::trace!("require_owned_page() frame exists for offset {frame:?}");

            if Frame::ptr_eq(frame, &THE_ZERO_FRAME) {
                // Clone the zero frame
                let new_frame = frame_alloc::alloc_one_zeroed()?;
                let old_frame = pages.replace(offset, new_frame.clone());
                log::trace!("old frame {old_frame:?}??");

                Ok(new_frame)
            } else {
                // -> IF FRAME IS OWNED BY VMO (how do we figure out the frame owner?)
                //     -> Return frame
                // -> IF DIFFERENT OWNER
                //     -> do copy on write
                //         -> allocate new frame
                //         -> copy from frame to new frame
                //         -> replace frame
                //         -> drop old frame clone (should free if refcount == 1)
                //         -> return new frame
                todo!()
            }
        } else {
            log::debug!("TODO request bytes from source (later when we actually have sources)");
            Err(Error::AccessDenied)
        }
    }

    pub fn require_read_page(&self, offset: usize) -> crate::Result<Frame> {
        let pages = self.pages.lock();

        if let Some(frame) = pages.get(offset) {
            log::trace!("require_read_page() frame exists for offset {frame:?}");

            // match page {
            //     Page::Frame(frame) => Ok(unsafe { frame.as_ref() }),
            //     Page::Zero => {
            //         todo!("clone the zero frame")
            //     }
            // }

            todo!()
        } else {
            log::debug!("TODO request bytes from source (later when we actually have sources)");
            Err(Error::AccessDenied)
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
