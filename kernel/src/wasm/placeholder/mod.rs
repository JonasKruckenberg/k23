//! Placeholder functionality for initial bootstrapping. This module will be replaced by k23-specific
//! code in the future. AS such, documentation is barebones, riddled with TODOs and probably only works
//! on macOS right now.

pub mod arch;
pub mod code_registry;
pub mod instance_allocator;
pub mod mmap;
mod setjmp;
pub(crate) mod signals;
pub mod trap_handling;

use core::num::NonZero;

/// Returns the host page size in bytes.
pub fn host_page_size() -> NonZero<usize> {
    // Safety: syscall
    unsafe {
        NonZero::new(
            usize::try_from(libc::sysconf(libc::_SC_PAGESIZE))
                .expect("host page size too big for usize"),
        )
        .expect("host page size is zero")
    }
}
