// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::FiberStack;
use crate::stack::{MIN_STACK_SIZE, StackPointer, StackTebFields};
use std::io::Error;
use std::ptr;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_GUARD, PAGE_READWRITE, VirtualAlloc, VirtualFree,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::Threading::SetThreadStackGuarantee;

pub struct DefaultFiberStack {
    top: StackPointer,
    bottom: usize,
    bottom_plus_guard: StackPointer,
    stack_guarantee: usize,
}

impl DefaultFiberStack {
    /// Creates a new stack which has at least the given capacity.
    pub fn new(size: usize) -> std::io::Result<Self> {
        // Apply minimum stack size.
        let size = size.max(MIN_STACK_SIZE);

        // Calculate how many extra pages we need to add for the various guard
        // pages:
        // - 1 or 2 guard pages to catch the fault (which may be 4095/8191 bytes
        //   into the guard page).
        // - N pages for the thread stack guarantee.
        // - 1 hard guard page at the end of the stack.
        let page_size = page_size();
        let guard_size = guard_page_size(page_size);
        let stack_guarantee = get_thread_stack_guarantee(page_size);
        let extra_pages = guard_size + stack_guarantee + page_size;

        // Add the extra pages to the requested size and round the size up to
        // a page boundary.
        let alloc_len = size
            .checked_add(extra_pages + page_size - 1)
            .expect("integer overflow while calculating stack size")
            & !(page_size - 1);

        unsafe {
            // Reserve virtual memory for the stack.
            let alloc_base = VirtualAlloc(ptr::null(), alloc_len, MEM_RESERVE, PAGE_READWRITE);
            if alloc_base.is_null() {
                return Err(Error::last_os_error());
            }

            // Create the result here. If the later VirtualAlloc calls fail then
            // this will be dropped and the memory will be unmapped.
            let alloc_top = alloc_base as usize + alloc_len;
            let limit = alloc_top - page_round_up(MIN_STACK_SIZE, page_size);
            let out = Self {
                top: StackPointer::new(alloc_top).unwrap(),
                bottom: limit,
                bottom_plus_guard: StackPointer::new(alloc_base as usize).unwrap(),
                stack_guarantee,
            };

            // Commit the first MIN_STACK_SIZE pages of the stack.
            if VirtualAlloc(
                limit as *mut _,
                alloc_top - limit,
                MEM_COMMIT,
                PAGE_READWRITE,
            )
            .is_null()
            {
                return Err(Error::last_os_error());
            }

            // Commit the guard pages.
            let stack_guard_size = guard_size + stack_guarantee;
            if VirtualAlloc(
                (limit - stack_guard_size) as *mut _,
                stack_guard_size,
                MEM_COMMIT,
                PAGE_READWRITE | PAGE_GUARD,
            )
            .is_null()
            {
                return Err(Error::last_os_error());
            }

            Ok(out)
        }
    }
}

impl Default for DefaultFiberStack {
    fn default() -> Self {
        Self::new(1024 * 1024).expect("failed to allocate stack")
    }
}

impl Drop for DefaultFiberStack {
    fn drop(&mut self) {
        unsafe {
            let alloc_base = self.bottom_plus_guard.get() as *mut _;
            let ret = VirtualFree(alloc_base, 0, MEM_RELEASE);
            debug_assert!(ret != 0);
        }
    }
}

unsafe impl FiberStack for DefaultFiberStack {
    #[inline]
    fn top(&self) -> StackPointer {
        self.top
    }

    #[inline]
    fn bottom(&self) -> StackPointer {
        self.bottom_plus_guard
    }

    #[inline]
    fn teb_fields(&self) -> StackTebFields {
        StackTebFields {
            StackTop: self.top.get(),
            StackBottom: self.bottom,
            StackBottomPlusGuard: self.bottom_plus_guard.get(),
            GuaranteedStackBytes: self.stack_guarantee,
        }
    }

    #[inline]
    fn update_teb_fields(&mut self, stack_limit: usize, guaranteed_stack_bytes: usize) {
        self.bottom = stack_limit;
        self.stack_guarantee = guaranteed_stack_bytes;
    }
}

fn page_size() -> usize {
    unsafe {
        let mut sysinfo: SYSTEM_INFO = std::mem::zeroed();
        GetSystemInfo(&mut sysinfo);
        assert!(sysinfo.dwPageSize.is_power_of_two());
        sysinfo.dwPageSize as usize
    }
}

fn page_round_up(val: usize, page_size: usize) -> usize {
    (val + page_size - 1) & !(page_size - 1)
}

fn get_thread_stack_guarantee(page_size: usize) -> usize {
    // Passing a value of 0 will just query the existing value.
    let mut stack_guarantee = 0;
    unsafe {
        SetThreadStackGuarantee(&mut stack_guarantee);
    }

    // At a bare minimum we need to reserve 1 page for the stack overflow
    // handler. Also round the guarantee up to a page boundary.
    page_round_up((stack_guarantee as usize).max(page_size), page_size)
}

fn guard_page_size(page_size: usize) -> usize {
    if cfg!(target_pointer_width = "64") {
        2 * page_size
    } else {
        page_size
    }
}
