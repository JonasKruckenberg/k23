#![allow(unused)]

use crate::machine_info::MachineInfo;
use crate::{arch, STACK_SIZE_PAGES};
use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use aspace::AddressSpace;
use core::alloc::Layout;
use core::any::Any;
use core::num::NonZeroUsize;
use core::ops::{Add, Range};
use core::{fmt, slice};
use loader_api::{BootInfo, MemoryRegionKind};
use mmu::frame_alloc::BuddyAllocator;
use mmu::{AddressRangeExt, Flush, PhysicalAddress, VirtualAddress};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use sync::{Mutex, OnceLock};
use xmas_elf::program::Type;

mod aspace;
mod mapping;

pub(crate) static FRAME_ALLOC: OnceLock<Mutex<BuddyAllocator>> = OnceLock::new();
pub static KERNEL_ASPACE: OnceLock<Mutex<AddressSpace>> = OnceLock::new();

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone)]
    pub struct PageFaultFlags: u8 {
        const WRITE = 1 << 0;
        const ACCESS = 1 << 1;
        const USER = 1 << 2;
        const INSTRUCTION = 1 << 3;
        /// fault originated from hardware
        const HW_FAULT = 1 << 4;
        /// fault originated from software
        const SW_FAULT = 1 << 5;
    }
}

pub fn init(boot_info: &BootInfo, minfo: &MachineInfo) -> crate::Result<()> {
    FRAME_ALLOC.get_or_init(|| unsafe {
        let usable_regions = boot_info
            .memory_regions()
            .iter()
            .filter(|region| region.kind.is_usable())
            .map(|region| region.range.clone());

        let alloc = BuddyAllocator::from_iter(usable_regions, boot_info.physical_address_offset);

        Mutex::new(alloc)
    });

    KERNEL_ASPACE.get_or_try_init(|| -> crate::Result<_> {
        let mut mmu_aspace = arch::vm::init(boot_info, minfo)?;

        let prng = ChaCha20Rng::from_seed(minfo.rng_seed.unwrap()[0..32].try_into().unwrap());
        let mut aspace = AddressSpace::new_kernel(mmu_aspace, prng);

        let mut flush = Flush::empty(0);
        reserve_wired_regions(&mut aspace, boot_info, &mut flush);
        flush.flush()?;

        for device in &minfo.mmio_devices {
            for region in &device.regions {
                let vmo = WiredVmo::new(region.clone());

                aspace.create_mapping(
                    region.clone().into_layout().unwrap(),
                    vmo,
                    0,
                    mmu::Flags::READ | mmu::Flags::WRITE,
                    device.name.to_string(),
                )?;
            }
        }

        Ok(Mutex::new(aspace))
    })?;

    Ok(())
}

// {
//     log::trace!("before:");
//     for m in aspace.tree.iter() {
//         log::trace!(
//             "{:<30} : {}..{} {:?}",
//             m.name,
//             m.range.start,
//             m.range.end,
//             m.flags
//         );
//     }
//
//     if let Some(rtc) = &minfo.rtc {
//         let vmo = WiredVmo::new(rtc.clone());
//         aspace.create_mapping(
//             rtc.clone().into_layout().unwrap(),
//             vmo,
//             0,
//             mmu::Flags::READ | mmu::Flags::WRITE,
//             "RTC".to_string(),
//         ).unwrap();
//     }
//
//     log::trace!("after:");
//     for m in aspace.tree.iter() {
//         log::trace!(
//             "{:<30} : {}..{} {:?}",
//             m.name,
//             m.range.start,
//             m.range.end,
//             m.flags
//         );
//     }
// }

fn reserve_wired_regions(
    aspace: &mut AddressSpace,
    boot_info: &BootInfo,
    flush: &mut Flush,
) -> crate::Result<()> {
    // reserve the physical memory map
    aspace.reserve(
        boot_info.physical_memory_map.clone(),
        mmu::Flags::READ | mmu::Flags::WRITE,
        "Physical Memory Map".to_string(),
        flush,
    )?;

    // reserve the allocated initial heap region
    if let Some(heap) = &boot_info.heap_region {
        aspace.reserve(
            heap.to_owned(),
            mmu::Flags::READ | mmu::Flags::WRITE,
            "Kernel Heap".to_string(),
            flush,
        )?;
    }

    // reserve the stack for each hart
    // TODO keep in sync with loader/vm.rs KernelAddressSpace::stack_region_for_hart
    // TODO account for guard pages
    let per_hart_stack_size = STACK_SIZE_PAGES as usize * arch::PAGE_SIZE;
    for hartid in 0..boot_info.hart_mask.count_ones() {
        let end = boot_info
            .stacks_region
            .end
            .sub(per_hart_stack_size * hartid as usize);

        aspace.reserve(
            end.sub(per_hart_stack_size)..end,
            mmu::Flags::READ | mmu::Flags::WRITE,
            format!("Hart {} Stack", hartid),
            flush,
        )?;
    }

    // reserve the TLS region if present
    if let Some(tls) = &boot_info.tls_region {
        aspace.reserve(
            tls.to_owned(),
            mmu::Flags::READ | mmu::Flags::WRITE,
            "Kernel TLS".to_string(),
            flush,
        )?;
    }

    let own_elf = unsafe {
        slice::from_ptr_range(
            boot_info
                .kernel_elf
                .clone()
                .add(boot_info.physical_address_offset.as_raw())
                .as_ptr_range(),
        )
    };
    let own_elf = xmas_elf::ElfFile::new(own_elf).unwrap();

    for ph in own_elf.program_iter() {
        if ph.get_type().unwrap() != Type::Load {
            continue;
        }

        let virt = boot_info.kernel_virt.start.add(ph.virtual_addr() as usize);

        let mut mmu_flags = mmu::Flags::empty();
        if ph.flags().is_read() {
            mmu_flags |= mmu::Flags::READ;
        }
        if ph.flags().is_write() {
            mmu_flags |= mmu::Flags::WRITE;
        }
        if ph.flags().is_execute() {
            mmu_flags |= mmu::Flags::EXECUTE;
        }

        assert!(
            !mmu_flags.contains(mmu::Flags::WRITE | mmu::Flags::EXECUTE),
            "elf segment (virtual range {:#x}..{:#x}) is marked as write-execute",
            ph.virtual_addr(),
            ph.virtual_addr() + ph.mem_size()
        );

        aspace.reserve(
            virt.align_down(arch::PAGE_SIZE)
                ..virt.add(ph.mem_size() as usize).align_up(arch::PAGE_SIZE),
            mmu_flags,
            format!("Kernel {mmu_flags} Segment"),
            flush,
        )?;
    }

    Ok(())
}

trait Vmo {
    // Returns true if the object is backed by a contiguous range of physical
    // memory.
    fn is_contiguous() -> bool;
    // Returns true if the object size can be changed.
    fn is_resizable() -> bool;
    // Returns true if the object's pages are discardable by the kernel.
    fn is_discardable() -> bool;
    fn commit_range(&mut self, range: Range<u64>);
    fn prefetch_range(&mut self, range: Range<u64>);
    fn decommit_range(&mut self, range: Range<u64>);
    fn zero_range(&mut self, range: Range<u64>);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[ktest::test]
    fn alloc_spot() {
        let mut kernel_aspace = crate::vm::KERNEL_ASPACE.get().unwrap().lock();

        for _ in 0..50 {
            kernel_aspace.find_spot(Layout::from_size_align(4096, 4096).unwrap(), 27);
        }
    }
}
