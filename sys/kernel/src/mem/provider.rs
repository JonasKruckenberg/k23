// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::sync::Arc;
use core::alloc::Layout;
use core::fmt::Debug;
use core::iter;
use core::num::NonZeroUsize;

use spin::{LazyLock, OnceLock};

use crate::arch;
use crate::mem::frame_alloc::{FRAME_ALLOC, Frame, FrameAllocator};
use crate::mem::frame_list::FrameList;

pub trait Provider: Debug {
    // TODO make async
    fn get_frame(&self, at_offset: usize, will_write: bool) -> crate::Result<Frame>;
    // TODO make async
    fn get_frames(
        &self,
        at_offset: usize,
        len: NonZeroUsize,
        will_write: bool,
    ) -> crate::Result<FrameList>;
    fn free_frame(&self, frame: Frame);
    fn free_frames(&self, frames: FrameList);
}

pub static THE_ZERO_FRAME: LazyLock<Arc<TheZeroFrame>> = LazyLock::new(|| {
    let frame_alloc = FRAME_ALLOC.get().unwrap();
    Arc::new(TheZeroFrame::new(frame_alloc))
});

#[derive(Debug)]
pub struct TheZeroFrame {
    frame_alloc: &'static FrameAllocator,
    frame: OnceLock<Frame>,
}

impl TheZeroFrame {
    pub fn new(frame_alloc: &'static FrameAllocator) -> Self {
        Self {
            frame_alloc,
            frame: OnceLock::new(),
        }
    }
    pub(super) fn frame(&self) -> &Frame {
        self.frame.get_or_init(|| {
            let frame = self.frame_alloc.alloc_one_zeroed().unwrap();
            tracing::trace!("THE_ZERO_FRAME: {}", frame.addr());
            frame
        })
    }
}

impl Provider for TheZeroFrame {
    fn get_frame(&self, _at_offset: usize, will_write: bool) -> crate::Result<Frame> {
        if will_write {
            self.frame_alloc.alloc_one_zeroed().map_err(Into::into)
        } else {
            Ok(self.frame().clone())
        }
    }

    fn get_frames(
        &self,
        _at_offset: usize,
        len: NonZeroUsize,
        will_write: bool,
    ) -> crate::Result<FrameList> {
        if will_write {
            let frames = self.frame_alloc.alloc_contiguous_zeroed(
                Layout::from_size_align(len.get(), arch::PAGE_SIZE).unwrap(),
            )?;

            let frames = FrameList::from_iter(frames.into_iter().map(|info| {
                // Safety: we just allocated the frame
                unsafe { Frame::from_free_info(info) }
            }));

            #[cfg(debug_assertions)]
            frames.assert_valid("TheZeroFrame::get_frames after allocation");

            Ok(frames)
        } else {
            Ok(FrameList::from_iter(iter::repeat_n(
                self.frame().clone(),
                len.get(),
            )))
        }
    }

    fn free_frame(&self, frame: Frame) {
        drop(frame);
    }

    fn free_frames(&self, frames: FrameList) {
        for frame in frames {
            self.free_frame(frame);
        }
    }
}
