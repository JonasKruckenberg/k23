use crate::error::Error;
use crate::frame_alloc::{Frame, FrameList};
use sync::Mutex;

#[derive(Debug)]
pub struct PagedVmo {
    pages: Mutex<FrameList>,
}

impl PagedVmo {
    pub(super) fn require_owned_page(&self, offset: usize) -> crate::Result<&Frame> {
        let pages = self.pages.lock();

        if let Some(frame) = pages.get(offset) {
            log::trace!("require_owned_page() frame exists for offset {frame:?}");

            // // if frame.addr() == unsafe { THE_ZERO_FRAME.get().unwrap().as_ref().phys } {
            //     let new_frame = FRAME_ALLOC
            //         .get()
            //         .unwrap()
            //         .allocate_one()
            //         .ok_or(Error::NoResources)?;
            //     let old_frame = pages.replace(offset, new_frame);
            //     log::trace!("TODO maybe free {old_frame:?}??");
            //
            //     Ok(unsafe { new_frame.as_ref() })
            // } else {
            //         // -> IF FRAME IS OWNED BY VMO (how do we figure out the frame owner?)
            //         //     -> Return frame
            //         // -> IF DIFFERENT OWNER
            //         //     -> do copy on write
            //         //         -> allocate new frame
            //         //         -> copy from frame to new frame
            //         //         -> replace frame
            //         //         -> drop old frame clone (should free if refcount == 1)
            //         //         -> return new frame
            todo!()
            // }
        } else {
            log::debug!("TODO request bytes from source (later when we actually have sources)");
            Err(Error::AccessDenied)
        }
    }

    pub(super) fn require_read_page(&self, offset: usize) -> crate::Result<&Frame> {
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
