// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::vm::frame_alloc::Frame;
use crate::vm::frame_list::FrameList;
use crate::vm::{frame_alloc, THE_ZERO_FRAME};
use core::range::Range;
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
    pub fn require_owned_frame(
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
                    .expect("newly allocated frame should be unique")
                    .as_mut_slice(phys_off);

                log::trace!(
                    "copying from {:?} to {:?}",
                    src.as_ptr_range(),
                    dst.as_ptr_range()
                );
                dst.copy_from_slice(src);
            }

            let new_frame = self.frames.insert(at_offset, new_frame.clone());
            Ok(new_frame)
        } else {
            todo!("TODO request bytes from source (later when we actually have sources)");
        }
    }

    pub fn require_read_frame(&self, at_offset: usize) -> crate::Result<&Frame> {
        if let Some(frame) = self.frames.get(at_offset) {
            Ok(frame)
        } else {
            todo!("TODO request bytes from source (later when we actually have sources)");
        }
    }

    pub fn free_frames(&mut self, range: Range<usize>) {
        let mut c = self.frames.cursor_mut(range.start);

        while c.offset() < range.end {
            let _frame = c.remove();

            c.move_next();
        }
    }
}
