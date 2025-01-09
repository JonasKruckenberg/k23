use crate::vm::frame_alloc::{Frame, FrameList};
use crate::vm::{frame_alloc, THE_ZERO_FRAME};
use sync::RwLock;
use crate::vm::frame_list::FrameState;

#[derive(Debug)]
pub struct PagedVmo {
    pages: RwLock<FrameList>,
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
        self.with_pages_mut(|pages| {
            if let Some(frame) = pages.get(offset) {
                match frame {
                    FrameState::Borrowed(_) => {}
                    FrameState::Owned(_) => {}
                    FrameState::Vacant => {}
                }
                
                
                todo!()
                
                // if frame.is_unique() {
                //     // we're the sole owner of the frame, so we're good to just return it
                //     Ok(frame.clone())
                // } else if Frame::ptr_eq(frame, &THE_ZERO_FRAME) {
                //     // the frame is the shared zero frame
                //     let new_frame = frame_alloc::alloc_one_zeroed()?;
                //     let old_frame = pages.replace(offset, new_frame.clone());
                //     log::trace!("old frame {old_frame:?}??");
                //     Ok(new_frame)
                // } else {
                //     // -> do copy on write
                //     //         //         -> allocate new frame
                //     //         //         -> copy from frame to new frame
                //     //         //         -> replace frame
                //     //         //         -> drop old frame clone (should free if refcount == 1)
                //     //         //         -> return new frame
                // 
                //     todo!()
                // }
            } else {
                todo!("TODO request bytes from source (later when we actually have sources)");
            }
        })
    }

    fn require_read_frame(&self, offset: usize) -> crate::Result<Frame> {
        self.with_pages(|pages| {
            if let Some(frame) = pages.get(offset) {
                log::trace!("require_read_page() frame exists for offset {frame:?}");

                // TODO zirkon clones the zero page here but idk if that necessary
                Ok(frame.clone())
            } else {
                todo!("TODO request bytes from source (later when we actually have sources)");
            }
        })
    }

    fn with_pages_mut<R>(&self, f: impl FnOnce(&mut FrameList) -> R) -> R {
        let mut pages = self.pages.write();

        // assert the intrusive list is valid before we operate on it
        #[cfg(debug_assertions)]
        pages.assert_valid();

        let r = f(&mut pages);

        // make sure the list is still valid after we're done with it
        #[cfg(debug_assertions)]
        pages.assert_valid();

        r
    }

    fn with_pages<R>(&self, f: impl FnOnce(&FrameList) -> R) -> R {
        let mut pages = self.pages.read();

        // assert the intrusive list is valid before we read it
        #[cfg(debug_assertions)]
        pages.assert_valid();

        f(&mut pages)
    }
}

impl FromIterator<Frame> for PagedVmo {
    fn from_iter<T: IntoIterator<Item = Frame>>(iter: T) -> Self {
        Self {
            pages: RwLock::new(FrameList::from_iter(iter)),
        }
    }
}