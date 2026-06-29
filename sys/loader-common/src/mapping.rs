// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ptr;
use core::range::Range;

use mem_core::{
    AddressRangeExt, FrameAllocator, MemoryAttributes, MemoryKind, PhysMap, PhysicalAddress,
    VirtualAddress, WriteOrExecute,
};
use mem_mmu::Flush;

use crate::kernel::{Permissions, RelocatedKernel};
use crate::{KernelAspaceLayout, arch};

/// Map every `PT_LOAD` segment of the relocated kernel image into `aspace`.
///
/// Each segment is mapped `staging_base + image_offset` -> `virt_base + image_offset`,
/// with permissions derived from its ELF flags. Gap pages between segments are
/// deliberately left unmapped. Modifications are recorded in `flush` but not
/// synchronized — the caller is responsible for flushing once all mapping is done.
pub fn map_kernel_image(
    aspace: &mut arch::KernelAspace,
    aspace_layout: &KernelAspaceLayout,
    kernel: &RelocatedKernel,
    physmap: &PhysMap,
    frame_alloc: &impl FrameAllocator,
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

        // Safety: aspace_layout ensures region is disjoint (unmapped)
        // and we explicitly aligned the ranges above.
        unsafe {
            aspace.map_contiguous(virt, phys, attrs, frame_alloc, physmap, flush)?;
        }
    }

    log::debug!("apply kernel RELRO...");
    protect_relro(aspace, aspace_layout, kernel, physmap, flush);
    log::debug!("applied kernel RELRO");

    Ok(())
}

fn protect_relro(
    aspace: &mut arch::KernelAspace,
    aspace_layout: &KernelAspaceLayout,
    kernel: &RelocatedKernel,
    physmap: &PhysMap,
    flush: &mut Flush,
) {
    // calculate and apply RELRO
    let relro = {
        let start = aspace_layout
            .kernel_image
            .start
            .add(kernel.relro_range().start)
            .align_down(aspace.granule_size());

        let end = aspace_layout
            .kernel_image
            .start
            .add(kernel.relro_range().end)
            // NB: glibc aligns-down BOTH boundaries
            // (https://elixir.bootlin.com/glibc/glibc-2.35/source/elf/dl-reloc.c#L346)
            .align_down(aspace.granule_size());

        Range::from(start..end)
    };

    // Safety: `map_kernel_image` ensures region is already mapped and
    // we ensured the alignment above
    unsafe {
        aspace.set_attributes(
            relro,
            MemoryAttributes::new().with(MemoryAttributes::READ, true),
            physmap,
            flush,
        );
    }
}

pub fn map_tls_block(
    aspace: &mut arch::KernelAspace,
    aspace_layout: &KernelAspaceLayout,
    boot_hart_tls: &[u8],
    physmap: &PhysMap,
    frame_alloc: &impl FrameAllocator,
    flush: &mut Flush,
) -> crate::Result<()> {
    let phys = PhysicalAddress::from_ptr(boot_hart_tls.as_ptr());

    let attrs = MemoryAttributes::new()
        .with(MemoryAttributes::READ, true)
        .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write);

    // Safety: aspace_layout ensures region is disjoint (unmapped)
    // and allocator ensures phys allocation is aligned
    unsafe {
        aspace.map_contiguous(
            aspace_layout.boot_hart_tls,
            phys,
            attrs,
            frame_alloc,
            physmap,
            flush,
        )?;
    }

    Ok(())
}

pub fn map_stack(
    aspace: &mut arch::KernelAspace,
    aspace_layout: &KernelAspaceLayout,
    boot_hart_stack: &[u8],
    physmap: &PhysMap,
    frame_alloc: &impl FrameAllocator,
    flush: &mut Flush,
) -> crate::Result<()> {
    let phys = PhysicalAddress::from_ptr(boot_hart_stack.as_ptr());

    let attrs = MemoryAttributes::new()
        .with(MemoryAttributes::READ, true)
        .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write);

    // Safety: aspace_layout ensures region is disjoint (unmapped)
    // and allocator ensures phys allocation is aligned
    unsafe {
        aspace.map_contiguous(
            aspace_layout.boot_hart_stack,
            phys,
            attrs,
            frame_alloc,
            physmap,
            flush,
        )?;
    }

    Ok(())
}

pub(crate) fn map_boot_info(
    aspace: &mut arch::KernelAspace,
    aspace_layout: &KernelAspaceLayout,
    boot_info: &mut loader_api::BootInfo,
    physmap: &PhysMap,
    frame_alloc: &impl FrameAllocator,
    flush: &mut Flush,
) -> crate::Result<()> {
    let phys = PhysicalAddress::from_ptr(ptr::from_mut(boot_info));

    let attrs = MemoryAttributes::new().with(MemoryAttributes::READ, true);

    // Safety: aspace_layout ensures region is disjoint (unmapped)
    // and allocator ensures phys allocation is aligned
    unsafe {
        aspace.map_contiguous(
            aspace_layout.boot_info,
            phys,
            attrs,
            frame_alloc,
            physmap,
            flush,
        )?;
    }

    Ok(())
}

pub(crate) fn map_physical_memory(
    aspace: &mut arch::KernelAspace,
    aspace_layout: &KernelAspaceLayout,
    physmap: &PhysMap,
    frame_alloc: &impl FrameAllocator,
    flush: &mut Flush,
) -> crate::Result<()> {
    let attrs = MemoryAttributes::new()
        .with(MemoryAttributes::READ, true)
        .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write);

    // Safety: aspace_layout ensures region is disjoint (unmapped)
    // and physmap ensures minimum alignment
    unsafe {
        aspace.map_contiguous(
            aspace_layout.physmap.range_virt(),
            aspace_layout.physmap.range_phys().start,
            attrs,
            frame_alloc,
            physmap,
            flush,
        )?;
    }

    Ok(())
}

/// Map the firmware console UART register block into the kernel address space
/// as device memory.
///
/// `phys` is the UART register block and `virt` the range reserved for it in
/// [`KernelAspaceLayout`]; both are page-aligned defensively before mapping, so
/// their lengths must match. Like [`map_physical_memory`] this maps an existing
/// device region — no frames are allocated for it.
pub(crate) fn map_uart(
    aspace: &mut arch::KernelAspace,
    virt: Range<VirtualAddress>,
    phys: Range<PhysicalAddress>,
    physmap: &PhysMap,
    frame_alloc: &impl FrameAllocator,
    flush: &mut Flush,
) -> crate::Result<()> {
    let granule = aspace.granule_size();
    let phys = phys.align_out(granule);
    let virt = virt.align_out(granule);
    debug_assert_eq!(virt.len(), phys.len());

    let attrs = MemoryAttributes::new()
        .with(MemoryAttributes::READ, true)
        .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write)
        .with(MemoryAttributes::KIND, MemoryKind::Device);

    log::debug!("mapping UART {virt:?} => {phys:?}");

    // Safety: `phys` is the firmware-described UART register block and `virt` is
    // a fresh, unmapped range reserved for it in the kernel layout, of equal size.
    unsafe {
        aspace.map_contiguous(virt, phys.start, attrs, frame_alloc, physmap, flush)?;
    }

    Ok(())
}

pub fn map_handoff_trampoline(
    aspace: &mut arch::KernelAspace,
    physmap: &PhysMap,
    frame_alloc: &impl FrameAllocator,
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

    log::debug!("handoff trampoline {start}..{end}");

    // Safety: page allocator ensures region is disjoint (unmapped) and we checked
    // the bounds to be aligned above.
    unsafe {
        aspace.map_identity(Range::from(start..end), attrs, frame_alloc, physmap, flush)?;
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
