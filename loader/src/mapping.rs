// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::alloc::Layout;
use core::ops::Range;
use core::{cmp, ptr, slice};

use bitflags::bitflags;
use fallible_iterator::FallibleIterator;
use kmem_core::bootstrap::BootstrapArch;
use kmem_core::{
    AddressRangeExt, AddressSpace, Arch, Flush, FrameAllocator, MemoryAttributes, PhysicalAddress,
    VirtualAddress, WriteOrExecute,
};
use loader_api::TlsTemplate;
use xmas_elf::dynamic::Tag;
use xmas_elf::program::{SegmentData, Type};
use xmas_elf::P64;

use crate::error::Error;
use crate::kernel::Kernel;
use crate::machine_info::MachineInfo;
use crate::page_alloc::PageAllocator;
use crate::{SelfRegions};

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq)]
    pub struct Flags: u8 {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
    }
}

pub fn identity_map_self<A, F>(
    aspace: &mut AddressSpace<BootstrapArch<A>>,
    frame_alloc: F,
    self_regions: &SelfRegions,
    flush: &mut Flush,
) -> crate::Result<()>
where
    A: Arch,
    F: FrameAllocator,
{
    log::trace!(
        "Identity mapping loader executable region {:#x?}...",
        self_regions.executable
    );
    unsafe {
        aspace.map_identity(
            self_regions.executable.clone(),
            MemoryAttributes::new()
                .with(MemoryAttributes::READ, true)
                .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Execute),
            frame_alloc.by_ref(),
            flush,
        )?;
    }

    log::trace!(
        "Identity mapping loader read-only region {:#x?}...",
        self_regions.read_only
    );
    unsafe {
        aspace.map_identity(
            self_regions.read_only.clone(),
            MemoryAttributes::new().with(MemoryAttributes::READ, true),
            frame_alloc.by_ref(),
            flush,
        )?;
    }

    log::trace!(
        "Identity mapping loader read-write region {:#x?}...",
        self_regions.read_write
    );
    unsafe {
        aspace.map_identity(
            self_regions.read_write.clone(),
            MemoryAttributes::new()
                .with(MemoryAttributes::READ, true)
                .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write),
            frame_alloc.by_ref(),
            flush,
        )?;
    }

    Ok(())
}

// pub fn map_physical_memory(
//     root_pgtable: PhysicalAddress,
//     frame_alloc: &mut FrameAllocator,
//     page_alloc: &mut PageAllocator,
//     minfo: &MachineInfo,
// ) -> crate::Result<(VirtualAddress, Range<VirtualAddress>)> {
//     let alignment = arch::page_size_for_level(2);
//
//     let phys = minfo.memory_hull().align_out(alignment);
//     let virt = Range {
//         start: arch::KERNEL_ASPACE_BASE.add(phys.start.get()),
//         end: arch::KERNEL_ASPACE_BASE.add(phys.end.get()),
//     };
//
//     debug_assert!(phys.start.is_aligned_to(alignment) && phys.end.is_aligned_to(alignment));
//     debug_assert!(virt.start.is_aligned_to(alignment) && virt.end.is_aligned_to(alignment));
//
//     log::trace!("Mapping physical memory {phys:?} => {virt:?}...");
//     // Safety: Leaving the address space in an invalid state here is fine since on panic we'll
//     // abort startup anyway
//     unsafe {
//         arch::map_contiguous(
//             root_pgtable,
//             frame_alloc,
//             virt.start,
//             phys.start,
//             phys.len(),
//             Flags::READ | Flags::WRITE,
//             VirtualAddress::MIN, // called before translation into higher half
//         )?;
//     }
//
//     // exclude the physical memory map region from page allocation
//     page_alloc.reserve(virt.start, phys.len());
//
//     Ok((arch::KERNEL_ASPACE_BASE, virt))
// }

pub fn map_kernel<A, F>(
    aspace: &mut AddressSpace<A>,
    frame_alloc: F,
    page_alloc: &mut PageAllocator,
    kernel: &Kernel,
    minfo: &MachineInfo,
    flush: &mut Flush,
) -> crate::Result<(Range<VirtualAddress>, Option<TlsAllocation>)>
where
    A: Arch,
    F: FrameAllocator,
{
    let kernel_virt = page_alloc.allocate(
        Layout::from_size_align(
            usize::try_from(kernel.mem_size())?,
            usize::try_from(kernel.max_align())?,
        )
        .unwrap(),
    )?;

    let mut maybe_tls_allocation = None;

    // Load the segments into virtual memory.
    for ph in kernel.elf_file.program_iter() {
        match ph.get_type().unwrap() {
            Type::Load => handle_load_segment(
                aspace,
                frame_alloc.by_ref(),
                &ProgramHeader::try_from(ph)?,
                kernel.base_phys,
                kernel_virt.start,
                flush,
            )?,
            Type::Tls => {
                let ph = ProgramHeader::try_from(ph)?;
                let old = maybe_tls_allocation.replace(handle_tls_segment(
                    aspace,
                    frame_alloc.by_ref(),
                    page_alloc,
                    &ph,
                    kernel_virt.start,
                    minfo,
                    flush,
                )?);
                log::trace!("{maybe_tls_allocation:?}");
                assert!(old.is_none(), "multiple TLS segments not supported");
            }
            _ => {}
        }
    }

    // Apply relocations in virtual memory.
    for ph in kernel.elf_file.program_iter() {
        if ph.get_type().unwrap() == Type::Dynamic {
            handle_dynamic_segment(
                &ProgramHeader::try_from(ph).unwrap(),
                &kernel.elf_file,
                kernel_virt.start,
            )?;
        }
    }

    //     // Mark some memory regions as read-only after relocations have been
    //     // applied.
    //     for ph in kernel.elf_file.program_iter() {
    //         if ph.get_type().unwrap() == Type::GnuRelro {
    //             handle_relro_segment(
    //                 aspace,
    //                 &ProgramHeader::try_from(ph).unwrap(),
    //                 kernel_virt.start,
    //                 flush,
    //             )?;
    //         }
    //     }

    Ok((kernel_virt, maybe_tls_allocation))
}

/// Map an ELF LOAD segment.
fn handle_load_segment<A, F>(
    aspace: &mut AddressSpace<A>,
    frame_alloc: F,
    ph: &ProgramHeader,
    phys_base: PhysicalAddress,
    virt_base: VirtualAddress,
    flush: &mut Flush,
) -> crate::Result<()>
where
    A: Arch,
    F: FrameAllocator,
{
    let attributes = attributes_for_segment(ph);

    log::trace!(
        "Handling Segment: LOAD off {offset:#016x} vaddr {vaddr:#016x} align {align} filesz {filesz:#016x} memsz {memsz:#016x} attributes {attributes:?}",
        offset = ph.offset,
        vaddr = ph.virtual_address,
        align = ph.align,
        filesz = ph.file_size,
        memsz = ph.mem_size
    );

    let phys: Range<PhysicalAddress> = Range::from_start_len(phys_base.add(ph.offset), ph.file_size).align_out(ph.align);

    let virt =
        Range::from_start_len(virt_base.add(ph.virtual_address), ph.file_size).align_out(ph.align);

    log::trace!("mapping {virt:#x?} => {phys:#x?}");
    // Safety: Leaving the address space in an invalid state here is fine since on panic we'll
    // abort startup anyway
    unsafe {
        aspace.map_contiguous(
            virt,
            phys_base.add(ph.offset),
            attributes,
            frame_alloc.by_ref(),
            flush,
        )?;
    }

    if ph.file_size < ph.mem_size {
        handle_bss_section(
            aspace,
            frame_alloc,
            ph,
            attributes,
            phys_base,
            virt_base,
            flush,
        )?;
    }

    Ok(())
}

/// BSS sections are special, since they take up virtual memory that is not present in the "physical" elf file.
///
/// Usually, this means just allocating zeroed frames and mapping them "in between" the pages
/// backed by the elf file. However, quite often the boundary between DATA and BSS sections is
/// *not* page aligned (since that would unnecessarily bloat the elf file) which means for us
/// that we need special handling for the last DATA page that is only partially filled with data
/// and partially filled with zeroes. Here's how we do this:
///
/// 1. We calculate the size of the segments zero initialized part.
/// 2. We then figure out whether the boundary is page-aligned or if there are DATA bytes we need to account for.
///    2.1. IF there are data bytes to account for, we allocate a zeroed frame,
///    2.2. we then copy over the relevant data from the DATA section into the new frame
///    2.3. and lastly replace last page previously mapped by `handle_load_segment` to stitch things up.
/// 3. If the BSS section is larger than that one page, we allocate additional zeroed frames and map them in.
fn handle_bss_section<A, F>(
    aspace: &mut AddressSpace<A>,
    frame_alloc: F,
    ph: &ProgramHeader,
    attributes: MemoryAttributes,
    phys_base: PhysicalAddress,
    virt_base: VirtualAddress,
    flush: &mut Flush,
) -> crate::Result<()>
where
    A: Arch,
    F: FrameAllocator,
{
    let virt_start = virt_base.add(ph.virtual_address);
    let zero_start = virt_start.add(ph.file_size);
    let zero_end = virt_start.add(ph.mem_size);

    let data_bytes_before_zero = zero_start.get() & 0xfff;

    log::trace!(
        "handling BSS {:#x?}, data bytes before {data_bytes_before_zero}",
        zero_start..zero_end
    );

    if data_bytes_before_zero != 0 {
        let last_page = virt_start
            .add(ph.file_size.saturating_sub(1))
            .align_down(ph.align);
        let last_frame = phys_base
            .add(ph.offset + ph.file_size - 1)
            .align_down(ph.align);

        let new_frame = frame_alloc
            .allocate_contiguous_zeroed(aspace.arch().memory_mode().page_layout(), aspace.arch())?;

        // Safety: we just allocated the frame
        unsafe {
            let src = slice::from_raw_parts(
                aspace.arch().phys_to_virt(last_frame).as_ptr(),
                data_bytes_before_zero,
            );

            let dst = slice::from_raw_parts_mut(
                aspace.arch().phys_to_virt(new_frame).as_mut_ptr(),
                data_bytes_before_zero,
            );

            log::trace!("copying {data_bytes_before_zero} bytes from {src:p} to {dst:p}...");
            ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
        }

        // Safety: Leaving the address space in an invalid state here is fine since on panic we'll
        // abort startup anyway
        unsafe {
            aspace.remap_contiguous(
                Range::from_start_len(last_page, aspace.arch().memory_mode().page_size()),
                last_frame,
                flush,
            );
        }
    }

    log::trace!("zero_start {zero_start:?} zero_end {zero_end:?}");
    // zero_start either lies at a page boundary OR somewhere within the first page
    // by aligning up, we move it to the beginning of the *next* page.
    let mut virt = Range {
        start: zero_start.align_up(ph.align),
        end: zero_end.align_up(ph.align),
    };

    if !virt.is_empty() {
        let frame_iter = frame_alloc.allocate_zeroed(
            Layout::from_size_align(virt.len(), aspace.arch().memory_mode().page_size()).unwrap(),
            aspace.arch(),
        );

        log::trace!("mapping additional zeros {virt:?}");
        unsafe {
            aspace.map(virt, frame_iter, attributes, frame_alloc, flush)?;
        }
    }

    Ok(())
}

fn handle_dynamic_segment(
    ph: &ProgramHeader,
    elf_file: &xmas_elf::ElfFile,
    virt_base: VirtualAddress,
) -> crate::Result<()> {
    log::trace!("parsing RELA info...");

    if let Some(rela_info) = ph.parse_rela(elf_file)? {
        // Safety: we have to trust the ELF data
        let relas = unsafe {
            #[expect(clippy::cast_ptr_alignment, reason = "this is fine")]
            let ptr = elf_file
                .input
                .as_ptr()
                .byte_add(usize::try_from(rela_info.offset)?)
                .cast::<xmas_elf::sections::Rela<P64>>();

            slice::from_raw_parts(ptr, usize::try_from(rela_info.count)?)
        };

        // TODO memory fence here

        log::trace!("applying relocations in virtual memory...");
        for rela in relas {
            apply_relocation(rela, virt_base);
        }
    }

    Ok(())
}

fn apply_relocation(rela: &xmas_elf::sections::Rela<P64>, virt_base: VirtualAddress) {
    assert_eq!(
        rela.get_symbol_table_index(),
        0,
        "relocations using the symbol table are not supported"
    );

    const R_RISCV_RELATIVE: u32 = 3;

    match rela.get_type() {
        R_RISCV_RELATIVE => {
            // Calculate address at which to apply the relocation.
            // dynamic relocations offsets are relative to the virtual layout of the elf,
            // not the physical file
            let target = virt_base.add(usize::try_from(rela.get_offset()).unwrap());

            // Calculate the value to store at the relocation target.
            let value = virt_base.offset(isize::try_from(rela.get_addend()).unwrap());

            // log::trace!("reloc R_RISCV_RELATIVE offset: {:#x}; addend: {:#x} => target {target:?} value {value:?}", rela.get_offset(), rela.get_addend());
            // Safety: we have to trust the ELF data here
            unsafe {
                target
                    .as_mut_ptr()
                    .cast::<usize>()
                    .write_unaligned(value.get());
            }
        }
        _ => unimplemented!("unsupported relocation type {}", rela.get_type()),
    }
}

/// Map the kernel thread-local storage (TLS) memory regions.
fn handle_tls_segment<A, F>(
    aspace: &mut AddressSpace<A>,
    frame_alloc: F,
    page_alloc: &mut PageAllocator,
    ph: &ProgramHeader,
    virt_base: VirtualAddress,
    minfo: &MachineInfo,
    flush: &mut Flush,
) -> crate::Result<TlsAllocation>
where
    A: Arch,
    F: FrameAllocator,
{
    let layout = Layout::from_size_align(
        ph.mem_size,
        cmp::max(ph.align, aspace.arch().memory_mode().page_size()),
    )
    .unwrap()
    .repeat(minfo.hart_mask.count_ones() as usize)
    .unwrap()
    .0
    .pad_to_align();
    log::trace!("allocating TLS segment {layout:?}...");

    let virt = page_alloc.allocate(layout)?;

    let frame_iter = frame_alloc.allocate_zeroed(layout, aspace.arch());

    log::trace!("Mapping TLS region {virt:?} ...",);
    // log::trace!(
    //     "Mapping TLS region {virt_start:?}..{:?} => {chunk:?} ...",
    //     virt_start.add(chunk.len())
    // );
    unsafe {
        aspace.map(
            virt.clone(),
            frame_iter,
            MemoryAttributes::new()
                .with(MemoryAttributes::READ, true)
                .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write),
            frame_alloc,
            flush,
        )?;
    }

    Ok(TlsAllocation {
        virt,
        template: TlsTemplate {
            start_addr: virt_base.add(ph.virtual_address),
            mem_size: align_up(
                ph.mem_size,
                cmp::max(ph.align, aspace.arch().memory_mode().page_size()),
            ),
            file_size: ph.file_size,
            align: ph.align,
        },
    })
}

#[derive(Debug)]
pub struct TlsAllocation {
    /// The TLS region in virtual memory
    virt: Range<VirtualAddress>,
    /// The template we allocated for
    pub template: TlsTemplate,
}

impl TlsAllocation {
    pub fn region_for_hart(&self, hartid: usize) -> Range<VirtualAddress> {
        let start = self.virt.start.add(self.template.mem_size * hartid);

        Range::from_start_len(start, self.template.mem_size)
    }

    pub fn initialize_for_hart(&self, hartid: usize) {
        if self.template.file_size != 0 {
            // Safety: We have to trust the loaders BootInfo here
            unsafe {
                let src: &[u8] = slice::from_raw_parts(
                    self.template.start_addr.as_mut_ptr(),
                    self.template.file_size,
                );
                let dst: &mut [u8] = slice::from_raw_parts_mut(
                    self.region_for_hart(hartid).start.as_mut_ptr(),
                    self.template.file_size,
                );

                // sanity check to ensure our destination allocated memory is actually zeroed.
                // if it's not, that likely means we're about to override something important
                debug_assert!(dst.iter().all(|&x| x == 0));

                dst.copy_from_slice(src);
            }
        }
    }
}

pub fn map_kernel_stacks<A, F>(
    aspace: &mut AddressSpace<A>,
    frame_alloc: F,
    page_alloc: &mut PageAllocator,
    minfo: &MachineInfo,
    per_cpu_size_pages: usize,
    flush: &mut Flush,
) -> crate::Result<StacksAllocation>
where
    A: Arch,
    F: FrameAllocator,
{
    let per_cpu_size = per_cpu_size_pages * aspace.arch().memory_mode().page_size();
    let per_cpu_size_with_guard = per_cpu_size + aspace.arch().memory_mode().page_size();

    let layout_with_guard = Layout::from_size_align(
        per_cpu_size_with_guard,
        aspace.arch().memory_mode().page_size(),
    )
    .unwrap()
    .repeat(minfo.hart_mask.count_ones() as usize)
    .unwrap()
    .0;

    let virt = page_alloc.allocate(layout_with_guard)?;
    log::trace!("Mapping stacks region {virt:#x?}...");

    for hart in 0..minfo.hart_mask.count_ones() {
        let layout =
            Layout::from_size_align(per_cpu_size, aspace.arch().memory_mode().page_size()).unwrap();

        let virt = Range::from_start_len(
            virt.end
                .add(per_cpu_size_with_guard * hart as usize)
                .sub(per_cpu_size),
            per_cpu_size,
        );

        log::trace!("Allocating stack {layout:?}...");
        // The stacks region doesn't need to be zeroed, since we will be filling it with
        // the canary pattern anyway
        let frame_iter = frame_alloc.allocate(layout);

        log::trace!("mapping stack for hart {hart} {virt:?}",);
        //     log::trace!(
        //         "mapping stack for hart {hart} {virt:?}..{:?} => {chunk:?}",
        //         virt.add(chunk.len())
        //     );
        unsafe {
            aspace.map(
                virt,
                frame_iter,
                MemoryAttributes::new()
                    .with(MemoryAttributes::READ, true)
                    .with(MemoryAttributes::WRITE_OR_EXECUTE, WriteOrExecute::Write),
                frame_alloc.by_ref(),
                flush,
            )?;
        }
    }

    Ok(StacksAllocation {
        virt,
        per_cpu_size,
        per_cpu_size_with_guard,
    })
}

pub struct StacksAllocation {
    /// The TLS region in virtual memory
    virt: Range<VirtualAddress>,
    per_cpu_size: usize,
    per_cpu_size_with_guard: usize,
}

impl StacksAllocation {
    pub fn region_for_cpu(&self, cpuid: usize) -> Range<VirtualAddress> {
        let end = self.virt.end.add(self.per_cpu_size_with_guard * cpuid);

        end.sub(self.per_cpu_size)..end
    }
}

struct ProgramHeader<'a> {
    pub p_flags: xmas_elf::program::Flags,
    pub align: usize,
    pub offset: usize,
    pub virtual_address: usize,
    pub file_size: usize,
    pub mem_size: usize,
    ph: xmas_elf::program::ProgramHeader<'a>,
}

impl ProgramHeader<'_> {
    pub fn parse_rela(&self, elf_file: &xmas_elf::ElfFile) -> crate::Result<Option<RelaInfo>> {
        let data = self.ph.get_data(elf_file).map_err(Error::Elf)?;
        let fields = match data {
            SegmentData::Dynamic32(_) => unimplemented!("32-bit elf files are not supported"),
            SegmentData::Dynamic64(fields) => fields,
            _ => return Ok(None),
        };

        let mut rela = None; // Address of Rela relocs
        let mut rela_size = None; // Total size of Rela relocs
        let mut rela_ent = None; // Size of one Rela reloc

        for field in fields {
            let tag = field.get_tag().map_err(Error::Elf)?;
            match tag {
                Tag::Rela => {
                    let ptr = field.get_ptr().map_err(Error::Elf)?;
                    let prev = rela.replace(ptr);
                    assert!(
                        prev.is_none(),
                        "Dynamic section contains more than one Rela entry"
                    );
                }
                Tag::RelaSize => {
                    let val = field.get_val().map_err(Error::Elf)?;
                    let prev = rela_size.replace(val);
                    assert!(
                        prev.is_none(),
                        "Dynamic section contains more than one RelaSize entry"
                    );
                }
                Tag::RelaEnt => {
                    let val = field.get_val().map_err(Error::Elf)?;
                    let prev = rela_ent.replace(val);
                    assert!(
                        prev.is_none(),
                        "Dynamic section contains more than one RelaEnt entry"
                    );
                }

                Tag::Rel | Tag::RelSize | Tag::RelEnt => {
                    panic!("REL relocations are not supported")
                }
                Tag::RelrSize | Tag::Relr | Tag::RelrEnt => {
                    panic!("RELR relocations are not supported")
                }
                _ => {}
            }
        }

        #[expect(clippy::manual_assert, reason = "cleaner this way")]
        if rela.is_none() && (rela_size.is_some() || rela_ent.is_some()) {
            panic!("Rela entry is missing but RelaSize or RelaEnt have been provided");
        }

        let Some(offset) = rela else {
            return Ok(None);
        };

        let total_size = rela_size.expect("RelaSize entry is missing");
        let entry_size = rela_ent.expect("RelaEnt entry is missing");

        Ok(Some(RelaInfo {
            offset,
            count: total_size / entry_size,
        }))
    }
}

struct RelaInfo {
    pub offset: u64,
    pub count: u64,
}

impl<'a> TryFrom<xmas_elf::program::ProgramHeader<'a>> for ProgramHeader<'a> {
    type Error = Error;

    fn try_from(ph: xmas_elf::program::ProgramHeader<'a>) -> Result<Self, Self::Error> {
        Ok(Self {
            p_flags: ph.flags(),
            align: usize::try_from(ph.align())?,
            offset: usize::try_from(ph.offset())?,
            virtual_address: usize::try_from(ph.virtual_addr())?,
            file_size: usize::try_from(ph.file_size())?,
            mem_size: usize::try_from(ph.mem_size())?,
            ph,
        })
    }
}

fn attributes_for_segment(ph: &ProgramHeader) -> MemoryAttributes {
    MemoryAttributes::new()
        .with(MemoryAttributes::READ, ph.p_flags.is_read())
        .with(
            MemoryAttributes::WRITE_OR_EXECUTE,
            match (ph.p_flags.is_write(), ph.p_flags.is_execute()) {
                (false, false) => WriteOrExecute::Neither,
                (true, false) => WriteOrExecute::Write,
                (false, true) => WriteOrExecute::Execute,
                (true, true) => panic!(
                    "elf segment (virtual range {:#x}..{:#x}) is marked as write-execute",
                    ph.virtual_address,
                    ph.virtual_address + ph.mem_size
                ),
            },
        )
}

#[must_use]
#[inline]
pub const fn align_up(num: usize, align: usize) -> usize {
    assert!(
        align.is_power_of_two(),
        "checked_align_up: align is not a power-of-two"
    );

    // SAFETY: `align` has been checked to be a power of 2 above
    let align_minus_one = unsafe { align.unchecked_sub(1) };

    let aligned = num.wrapping_add(align_minus_one) & 0usize.wrapping_sub(align);
    debug_assert!(aligned.is_multiple_of(align));
    debug_assert!(aligned >= num);
    aligned
}
