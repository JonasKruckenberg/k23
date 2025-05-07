// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::stack::valgrind::ValgrindStackRegistration;
use crate::stack::{FiberStack, MIN_STACK_SIZE, StackPointer};
use std::io::Error;
use std::mem::ManuallyDrop;
use std::ptr;

pub struct DefaultFiberStack {
    top: StackPointer,
    mmap_len: usize,
    valgrind: ManuallyDrop<ValgrindStackRegistration>,
}

impl DefaultFiberStack {
    /// Creates a new stack which has at least the given capacity.
    pub fn new(size: usize) -> std::io::Result<Self> {
        // Apply minimum stack size.
        let size = size.max(MIN_STACK_SIZE);

        // Add a guard page to the requested size and round the size up to
        // a page boundary.
        let page_size = page_size();
        let mmap_len = size
            .checked_add(page_size + page_size - 1)
            .expect("integer overflow while calculating stack size")
            & !(page_size - 1);

        // OpenBSD requires MAP_STACK on anything that is used as a stack.
        cfg_if::cfg_if! {
            if #[cfg(target_os = "openbsd")] {
                let map_flags = libc::MAP_ANONYMOUS | libc::MAP_PRIVATE | libc::MAP_STACK;
            } else {
                let map_flags = libc::MAP_ANONYMOUS | libc::MAP_PRIVATE;
            }
        }

        unsafe {
            // Reserve some address space for the stack.
            let mmap = libc::mmap(ptr::null_mut(), mmap_len, libc::PROT_NONE, map_flags, -1, 0);
            if mmap == libc::MAP_FAILED {
                return Err(Error::last_os_error());
            }

            // Create the result here. If the mprotect call fails then this will
            // be dropped and the memory will be unmapped.
            let out = Self {
                top: StackPointer::new(mmap as usize + mmap_len).unwrap(),
                mmap_len,
                valgrind: ManuallyDrop::new(ValgrindStackRegistration::new(
                    mmap as *mut u8,
                    mmap_len,
                )),
            };

            // Make everything except the guard page writable.
            if libc::mprotect(
                mmap.cast::<u8>().add(page_size).cast(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
            ) != 0
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
            // De-register the stack first.
            ManuallyDrop::drop(&mut self.valgrind);

            let mmap = self.top.get() - self.mmap_len;
            let ret = libc::munmap(mmap as _, self.mmap_len);
            debug_assert_eq!(ret, 0);
        }
    }
}

unsafe impl FiberStack for DefaultFiberStack {
    fn top(&self) -> StackPointer {
        self.top
    }

    fn bottom(&self) -> StackPointer {
        StackPointer::new(self.top.get() - self.mmap_len).unwrap()
    }
}

fn page_size() -> usize {
    let pagesize = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
    assert!(pagesize.is_power_of_two());
    pagesize
}
