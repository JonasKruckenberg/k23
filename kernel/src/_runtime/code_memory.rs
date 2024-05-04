//! Own a region of memory allocated to it
//! and manages its virtual memory and permissions

use core::ops::Range;
use vmm::VirtualAddress;

struct CodeMemory {
    /// The region of memory this `CodeMemory` owns
    region: Range<VirtualAddress>,
}

impl CodeMemory {
    pub fn new() {}

    /// Publishes the internal ELF image to be ready for execution.
    pub fn publish() {
        // apply relocations
        // Change page protections from read/write/supervisor to read/execute/userspace.
        // Flush caches
    }
}
