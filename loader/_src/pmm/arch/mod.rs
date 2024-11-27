// use core::num::NonZeroUsize;
// use crate::pmm::{FrameAllocator, PhysicalAddress, VirtualAddress};
// 
// cfg_if::cfg_if! {
//     if #[cfg(target_arch = "riscv64")] {
//         mod riscv64;
//         pub use riscv64::*;
//     }
// }

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq)]
    pub struct Flags: u8 {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
    }
}

pub trait PhysicalMemory {
    fn map(
        &mut self,
        frame_alloc: &mut dyn FrameAllocator,
        virt: VirtualAddress,
        phys: PhysicalAddress,
        len: usize,
        flags: Flags,
    );
    fn protect(&mut self, virt: VirtualAddress, len: usize, flags: Flags);
}
