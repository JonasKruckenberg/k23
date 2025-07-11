// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ptr::NonNull;
use core::sync::atomic::AtomicUsize;

use cordyceps::stack;
use pin_project::pin_project;

use crate::PhysicalAddress;

pub struct Frame {
    ptr: NonNull<FrameInner>,
}

#[pin_project(!Unpin)]
struct FrameInner {
    /// Links to other frames in a freelist either a global `Arena` or cpu-local page cache.
    #[pin]
    links: stack::Links<FrameInner>,
    /// Number of references to this frame, zero indicates a free frame.
    refcount: AtomicUsize,
    /// The physical address of the frame.
    addr: PhysicalAddress,
}
