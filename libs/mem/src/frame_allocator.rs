// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

mod frame;

use core::alloc::Layout;
use core::fmt;

#[derive(Debug)]
pub struct FrameAllocator {}

/// Allocation failure that may be due to resource exhaustion or invalid combination of arguments
/// such as a too-large alignment. Importantly this error is *not-permanent*, a caller choosing to
/// retry allocation at a later point in time or with different arguments and might receive a successful
/// result.
#[derive(Debug)]
pub struct AllocError;

// ===== impl FrameAllocator =====

impl FrameAllocator {
    pub fn max_alignment(&self) -> usize {
        // self.max_alignment

        todo!()
    }

    /// Allocate a single [`Frame`].
    pub fn alloc_one(&self) -> Result<Frame, AllocError> {
        todo!()
    }

    /// Allocate a single [`Frame`] and ensure the backing physical memory is zero initialized.
    pub fn alloc_one_zeroed(&self) -> Result<Frame, AllocError> {
        todo!()
    }

    /// Allocate a contiguous run of [`Frame`]s meeting the size and alignment requirements of `layout`.
    pub fn alloc_contiguous(&self, layout: Layout) -> Result<FrameList, AllocError> {
        todo!()
    }

    /// Allocate a contiguous run of [`Frame`]s meeting the size and alignment requirements of `layout`
    /// and ensuring the backing physical memory is zero initialized.
    pub fn alloc_contiguous_zeroed(&self, layout: Layout) -> Result<FrameList, AllocError> {
        todo!()
    }
}

// ===== impl AllocError =====

impl fmt::Display for AllocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AllocError")
    }
}

impl core::error::Error for AllocError {}
