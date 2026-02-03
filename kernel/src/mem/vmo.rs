// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::sync::Arc;
use core::ops::Range;

use anyhow::ensure;
use kmem::{AddressRangeExt, PhysicalAddress};
use kspin::RwLock;

use crate::arch;
use crate::mem::frame_alloc::frame_list::{Entry, FrameList};
use crate::mem::frame_alloc::{Frame, FrameAllocator};
use crate::mem::provider::{Provider, THE_ZERO_FRAME};

#[derive(Debug)]
pub enum Vmo {
    Wired,
    Phys(PhysVmo),
    Paged(RwLock<PagedVmo>),
}

impl Vmo {
    pub fn new_wired() -> Self {
        Self::Wired
    }

    pub fn new_phys(range: Range<PhysicalAddress>) -> Self {
        Self::Phys(PhysVmo { range })
    }

    pub fn new_zeroed(frame_alloc: &'static FrameAllocator) -> Self {
        Self::Paged(RwLock::new(PagedVmo {
            frames: FrameList::new(),
            provider: THE_ZERO_FRAME.clone(),
            frame_alloc,
        }))
    }

    pub fn is_valid_offset(&self, offset: usize) -> bool {
        match self {
            Vmo::Wired => unreachable!(),
            Vmo::Phys(vmo) => vmo.is_valid_offset(offset),
            Vmo::Paged(vmo) => vmo.read().is_valid_offset(offset),
        }
    }
}

#[derive(Debug)]
pub struct PhysVmo {
    range: Range<PhysicalAddress>,
}

impl PhysVmo {
    pub fn is_valid_offset(&self, offset: usize) -> bool {
        offset <= self.range.len()
    }

    pub fn lookup_contiguous(&self, range: Range<usize>) -> crate::Result<Range<PhysicalAddress>> {
        ensure!(
            range.start.is_multiple_of(arch::PAGE_SIZE),
            "range is not arch::PAGE_SIZE aligned"
        );
        let start = self.range.start.add(range.start);
        let end = self.range.start.add(range.end);

        ensure!(
            self.range.start <= start && self.range.end >= end,
            "requested range {range:?} is out of bounds for {:?}",
            self.range
        );

        Ok(start..end)
    }
}

#[derive(Debug)]
pub struct PagedVmo {
    frames: FrameList,
    provider: Arc<dyn Provider + Send + Sync>,
    frame_alloc: &'static FrameAllocator,
}

impl PagedVmo {
    pub fn is_valid_offset(&self, offset: usize) -> bool {
        offset <= self.frames.size()
    }

    pub fn require_owned_frame(&mut self, at_offset: usize) -> crate::Result<&mut Frame> {
        if let Some(old_frame) = self.frames.get(at_offset) {
            // we already have a unique frame reference, a write page fault against it shouldn't happen
            assert!(!old_frame.is_unique());

            tracing::trace!("require_owned_frame for resident frame, allocating new...");

            let mut new_frame = self.frame_alloc.alloc_one_zeroed()?;

            // If `old_frame` is the zero frame we don't need to copy any data around, it's
            // all zeroes anyway
            if !Frame::ptr_eq(old_frame, THE_ZERO_FRAME.frame()) {
                tracing::trace!("performing copy-on-write...");
                let src = old_frame.as_slice();
                let dst = Frame::get_mut(&mut new_frame)
                    .expect("newly allocated frame should be unique")
                    .as_mut_slice();

                tracing::trace!(
                    "copying from {:?} to {:?}",
                    src.as_ptr_range(),
                    dst.as_ptr_range()
                );
                dst.copy_from_slice(src);
            }

            let new_frame = self.frames.insert(at_offset, new_frame.clone());
            Ok(new_frame)
        } else {
            let new_frame = self.provider.get_frame(at_offset, true)?;
            debug_assert!(new_frame.is_unique());
            let new_frame = self.frames.insert(at_offset, new_frame);
            Ok(new_frame)
        }
    }

    pub fn require_read_frame(&mut self, at_offset: usize) -> crate::Result<&Frame> {
        let frame = match self.frames.entry(at_offset) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let new_frame = self.provider.get_frame(at_offset, false)?;
                entry.insert(new_frame)
            }
        };

        Ok(frame)
    }

    pub fn free_frames(&mut self, range: Range<usize>) {
        let mut c = self.frames.cursor_mut(range.start);

        while c.offset() < range.end {
            // TODO use `Provider::free_frames` here
            if let Some(frame) = c.remove() {
                self.provider.free_frame(frame);
            }

            c.move_next();
        }
    }
}
