// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::vm::{frame_alloc, Error, Frame, FrameList};
use alloc::sync::Arc;
use core::fmt::Debug;
use core::iter;
use core::num::NonZeroUsize;
use sync::LazyLock;

pub trait Provider: Debug {
    // TODO make async
    fn get_frame(&self, at_offset: usize) -> Result<Frame, Error>;
    // TODO make async
    fn get_frames(&self, at_offset: usize, len: NonZeroUsize) -> Result<FrameList, Error>;
    fn free_frame(&self, frame: Frame);
    fn free_frames(&self, frames: FrameList);
}

#[expect(tail_expr_drop_order, reason = "")]
pub static THE_ZERO_FRAME: LazyLock<Arc<TheZeroFrame>> = LazyLock::new(|| {
    let frame = frame_alloc::alloc_one_zeroed().unwrap();
    tracing::trace!("THE_ZERO_FRAME: {}", frame.addr());
    Arc::new(TheZeroFrame(frame))
});

#[derive(Debug, Clone)]
pub struct TheZeroFrame(Frame);

impl TheZeroFrame {
    pub(super) fn frame(&self) -> &Frame {
        &self.0
    }
}

impl Provider for TheZeroFrame {
    fn get_frame(&self, _at_offset: usize) -> Result<Frame, Error> {
        Ok(self.0.clone())
    }

    fn get_frames(&self, _at_offset: usize, len: NonZeroUsize) -> Result<FrameList, Error> {
        Ok(FrameList::from_iter(iter::repeat_n(
            self.0.clone(),
            len.get(),
        )))
    }

    fn free_frame(&self, frame: Frame) {
        debug_assert!(
            Frame::ptr_eq(&frame, &self.0),
            "attempted to free unrelated frame with the zero frame provider"
        );
        drop(frame);
    }

    fn free_frames(&self, frames: FrameList) {
        for frame in frames {
            self.free_frame(frame);
        }
    }
}
