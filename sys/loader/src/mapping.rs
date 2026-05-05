// Copyright 2026 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ptr;
use core::range::Range;

use mem_core::{
    AddressRangeExt, Flush, MemoryAttributes, PhysMap, PhysicalAddress, VirtualAddress,
    WriteOrExecute,
};

use crate::frame_alloc::UefiFrameAlloc;
use crate::kernel::{Permissions, RelocatedKernel};
use crate::{KernelAspaceLayout, arch};

/// Map every `PT_LOAD` segment of the relocated kernel image into `aspace`.
///
/// Each segment is mapped `staging_base + image_offset` -> `virt_base + image_offset`,
/// with permissions derived from its ELF flags. Gap pages between segments are
/// deliberately left unmapped. Modifications are recorded in `flush` but not
/// synchronized — the caller is responsible for flushing once all mapping is done.
pub fn map_kernel_image(
    aspace: &mut arch::InProgressKernelAspace,
    aspace_layout: &KernelAspaceLayout,
    kernel: &RelocatedKernel,
    physmap: &PhysMap,
    flush: &mut Flush,
) -> crate::Result<()> {
    let granule = aspace.granule_size();
    debug_assert!(
        aspace_layout.kernel_image.start.is_aligned_to(granule)
            && aspace_layout.kernel_image.end.is_aligned_to(granule)
    );

    for seg in kernel.load_segments() {
        let span = seg.unaligned_mem_range();
        let virt = Range::from_start_len(
            aspace_layout.kernel_image.start.add(span.start),
            span.end.checked_sub(span.start).unwrap(),
        )
        .align_out(granule);
        let phys = kernel.phys_base().add(span.start).align_down(granule);
        let attrs = segment_attributes(seg.perms);

        log::debug!("mapping {virt:?} => {phys} {attrs:?}");

        unsafe {
            aspace.map_contiguous(virt.into(), phys, attrs, UefiFrameAlloc, physmap, flush)?;
        }
    }

    Ok(())
}

pub fn map_tls_block(
    aspace: &mut arch::InProgressKernelAspace,
    aspace_layout: &KernelAspaceLayout,
    boot_hart_tls: &[u8],
    physmap: &PhysMap,
    flush: &mut Flush,
) -> crate::Result<()> {
    let phys = PhysicalAddress::from_ptr(boot_hart_tls.as_ptr());

    let attrs = MemoryAttributes::new()
        .with(MemoryAttributes::READ, true)
        .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write);

    unsafe {
        aspace.map_contiguous(
            aspace_layout.boot_hart_tls.into(),
            phys,
            attrs,
            UefiFrameAlloc,
            physmap,
            flush,
        )?;
    }

    Ok(())
}

pub fn map_stack(
    aspace: &mut arch::InProgressKernelAspace,
    aspace_layout: &KernelAspaceLayout,
    boot_hart_stack: &[u8],
    physmap: &PhysMap,
    flush: &mut Flush,
) -> crate::Result<()> {
    let phys = PhysicalAddress::from_ptr(boot_hart_stack.as_ptr());

    let attrs = MemoryAttributes::new()
        .with(MemoryAttributes::READ, true)
        .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write);

    unsafe {
        aspace.map_contiguous(
            aspace_layout.boot_hart_stack.into(),
            phys,
            attrs,
            UefiFrameAlloc,
            physmap,
            flush,
        )?;
    }

    Ok(())
}

pub(crate) fn map_boot_info(
    aspace: &mut arch::InProgressKernelAspace,
    aspace_layout: &KernelAspaceLayout,
    boot_info: &mut loader_api::BootInfo,
    physmap: &PhysMap,
    flush: &mut Flush,
) -> crate::Result<()> {
    let phys = PhysicalAddress::from_ptr(ptr::from_mut(boot_info));

    let attrs = MemoryAttributes::new().with(MemoryAttributes::READ, true);

    unsafe {
        aspace.map_contiguous(
            aspace_layout.boot_info.into(),
            phys,
            attrs,
            UefiFrameAlloc,
            physmap,
            flush,
        )?
    }

    Ok(())
}

pub(crate) fn map_physical_memory(
    aspace: &mut arch::InProgressKernelAspace,
    aspace_layout: &KernelAspaceLayout,
    physmap: &PhysMap,
    flush: &mut Flush,
) -> crate::Result<()> {
    let attrs = MemoryAttributes::new()
        .with(MemoryAttributes::READ, true)
        .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write);

    unsafe {
        aspace.map_contiguous(
            aspace_layout.physmap.range_virt().into(),
            aspace_layout.physmap.range_phys().start,
            attrs,
            UefiFrameAlloc,
            physmap,
            flush,
        )?
    }

    Ok(())
}

pub fn map_handoff_trampoline(
    aspace: &mut arch::InProgressKernelAspace,
    physmap: &PhysMap,
    flush: &mut Flush,
) -> crate::Result<Range<VirtualAddress>> {
    unsafe extern "C" {
        static __handoff_trampoline_start: u8;
        static __handoff_trampoline_end: u8;
    }

    let start = PhysicalAddress::from_ptr(&raw const __handoff_trampoline_start);
    let end = PhysicalAddress::from_ptr(&raw const __handoff_trampoline_end);

    assert!(start.is_aligned_to(aspace.granule_size()));
    assert!(end.is_aligned_to(aspace.granule_size()));

    let attrs = MemoryAttributes::new()
        .with(MemoryAttributes::READ, true)
        .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Execute);

    unsafe {
        aspace.map_identity(
            Range::from(start..end),
            attrs,
            UefiFrameAlloc,
            physmap,
            flush,
        )?
    }

    Ok(Range::from(
        VirtualAddress::new(start.get())..VirtualAddress::new(end.get()),
    ))
}

/// Translate ELF segment [`Permissions`] into hardware [`MemoryAttributes`].
fn segment_attributes(perms: Permissions) -> MemoryAttributes {
    let attrs = MemoryAttributes::new().with(MemoryAttributes::READ, true);
    match perms {
        Permissions::ReadOnly => attrs,
        Permissions::ReadWrite => {
            attrs.with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write)
        }
        Permissions::ReadExecute => {
            attrs.with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Execute)
        }
    }
}
