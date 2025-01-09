// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::error::Error;
use crate::kernel::Kernel;
use crate::machine_info::MachineInfo;
use crate::page_alloc::PageAllocator;
use crate::{arch, SelfRegions};
use core::alloc::Layout;
use core::num::NonZeroUsize;
use core::range::Range;
use core::{ptr, slice};
use loader_api::TlsTemplate;
use mmu::arch::PAGE_SIZE;
use mmu::frame_alloc::{BootstrapAllocator, FrameAllocator, NonContiguousFrames};
use mmu::{AddressRangeExt, AddressSpace, Flush, PhysicalAddress, VirtualAddress};
use xmas_elf::dynamic::Tag;
use xmas_elf::program::{SegmentData, Type};
use xmas_elf::P64;

pub fn identity_map_self(
    aspace: &mut AddressSpace,
    frame_alloc: &mut BootstrapAllocator,
    self_regions: &SelfRegions,
    flush: &mut Flush,
) -> crate::Result<()> {
    log::trace!(
        "Identity mapping loader executable region {:?}...",
        self_regions.executable
    );
    identity_map_range(
        aspace,
        frame_alloc,
        self_regions.executable.clone(),
        mmu::Flags::READ | mmu::Flags::EXECUTE,
        flush,
    )?;

    log::trace!(
        "Identity mapping loader read-only region {:?}...",
        self_regions.read_only
    );
    identity_map_range(
        aspace,
        frame_alloc,
        self_regions.read_only.clone(),
        mmu::Flags::READ,
        flush,
    )?;

    log::trace!(
        "Identity mapping loader read-write region {:?}...",
        self_regions.read_write
    );
    identity_map_range(
        aspace,
        frame_alloc,
        self_regions.read_write.clone(),
        mmu::Flags::READ | mmu::Flags::WRITE,
        flush,
    )?;

    Ok(())
}

#[inline]
fn identity_map_range(
    aspace: &mut AddressSpace,
    frame_alloc: &mut dyn FrameAllocator,
    phys: Range<PhysicalAddress>,
    flags: mmu::Flags,
    flush: &mut Flush,
) -> crate::Result<()> {
    let virt = VirtualAddress::new(phys.start.get()).unwrap();
    let len = NonZeroUsize::new(phys.size()).unwrap();

    aspace
        .map_contiguous(frame_alloc, virt, phys.start, len, flags, flush)
        .map_err(Into::into)
}

pub fn map_physical_memory(
    aspace: &mut AddressSpace,
    frame_alloc: &mut BootstrapAllocator,
    page_alloc: &mut PageAllocator,
    minfo: &MachineInfo,
    flush: &mut Flush,
) -> crate::Result<(VirtualAddress, Range<VirtualAddress>)> {
    let alignment = mmu::arch::page_size_for_level(2);

    let phys = minfo.memory_hull().checked_align_out(alignment).unwrap();
    let virt = Range::from(
        VirtualAddress::from_phys(phys.start, arch::KERNEL_ASPACE_BASE).unwrap()
            ..VirtualAddress::from_phys(phys.end, arch::KERNEL_ASPACE_BASE).unwrap(),
    );

    debug_assert!(phys.start.is_aligned_to(alignment) && phys.end.is_aligned_to(alignment));
    debug_assert!(virt.start.is_aligned_to(alignment) && virt.end.is_aligned_to(alignment));
    debug_assert_eq!(phys.size(), virt.size());

    log::trace!("Mapping physical memory {phys:?} => {virt:?}...",);
    aspace.map_contiguous(
        frame_alloc,
        virt.start,
        phys.start,
        NonZeroUsize::new(phys.size()).unwrap(),
        mmu::Flags::READ | mmu::Flags::WRITE,
        flush,
    )?;

    // exclude the physical memory map region from page allocation
    page_alloc.reserve(virt.start, phys.size());

    Ok((arch::KERNEL_ASPACE_BASE, virt))
}

pub fn map_kernel(
    aspace: &mut AddressSpace,
    frame_alloc: &mut dyn FrameAllocator,
    page_alloc: &mut PageAllocator,
    kernel: &Kernel,
    flush: &mut Flush,
) -> crate::Result<(Range<VirtualAddress>, Option<TlsTemplate>)> {
    let kernel_virt = page_alloc.allocate(
        Layout::from_size_align(kernel.mem_size() as usize, kernel.max_align() as usize).unwrap(),
    );

    let phys_base = PhysicalAddress::new(
        kernel.elf_file.input.as_ptr() as usize - aspace.physical_memory_offset().get(),
    );
    assert!(
        phys_base.is_aligned_to(PAGE_SIZE),
        "Loaded ELF file is not sufficiently aligned"
    );

    let mut maybe_tls_allocation = None;

    // Load the segments into virtual memory.
    for ph in kernel.elf_file.program_iter() {
        match ph.get_type().unwrap() {
            Type::Load => handle_load_segment(
                aspace,
                frame_alloc,
                &ProgramHeader::try_from(ph)?,
                phys_base,
                kernel_virt.start,
                flush,
            )?,
            Type::Tls => {
                let ph = ProgramHeader::try_from(ph)?;
                let old = maybe_tls_allocation.replace(TlsTemplate {
                    start_addr: kernel_virt.start.checked_add(ph.virtual_address).unwrap(),
                    mem_size: ph.mem_size,
                    file_size: ph.file_size,
                    align: ph.align,
                });
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

    // Mark some memory regions as read-only after relocations have been
    // applied.
    for ph in kernel.elf_file.program_iter() {
        if ph.get_type().unwrap() == Type::GnuRelro {
            handle_relro_segment(
                aspace,
                &ProgramHeader::try_from(ph).unwrap(),
                kernel_virt.start,
                flush,
            )?;
        }
    }

    Ok((kernel_virt, maybe_tls_allocation))
}

/// Map an ELF LOAD segment.
fn handle_load_segment(
    aspace: &mut AddressSpace,
    frame_alloc: &mut dyn FrameAllocator,
    ph: &ProgramHeader,
    phys_base: PhysicalAddress,
    virt_base: VirtualAddress,
    flush: &mut Flush,
) -> crate::Result<()> {
    let flags = flags_for_segment(ph);

    log::debug!(
        "Handling Segment: LOAD off {offset:#016x} vaddr {vaddr:#016x} align {align} filesz {filesz:#016x} memsz {memsz:#016x} flags {flags:?}",
        offset = ph.offset,
        vaddr = ph.virtual_address,
        align = ph.align,
        filesz = ph.file_size,
        memsz = ph.mem_size
    );

    let phys = {
        let start = phys_base.checked_add(ph.offset).unwrap();
        let end = start.checked_add(ph.file_size).unwrap();

        Range::from(start..end).checked_align_out(ph.align).unwrap()
    };

    let virt = {
        let start = virt_base.checked_add(ph.virtual_address).unwrap();
        let end = start.checked_add(ph.file_size).unwrap();

        Range::from(start..end).checked_align_out(ph.align).unwrap()
    };

    log::trace!("mapping {virt:?} => {phys:?}");
    aspace.map_contiguous(
        frame_alloc,
        virt.start,
        phys.start,
        NonZeroUsize::new(phys.size()).unwrap(),
        flags,
        flush,
    )?;

    if ph.file_size < ph.mem_size {
        handle_bss_section(aspace, frame_alloc, ph, flags, phys_base, virt_base, flush)?;
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
///     2.1. IF there are data bytes to account for, we allocate a zeroed frame,
///     2.2. we then copy over the relevant data from the DATA section into the new frame
///     2.3. and lastly replace last page previously mapped by `handle_load_segment` to stitch things up.
/// 3. If the BSS section is larger than that one page, we allocate additional zeroed frames and map them in.
fn handle_bss_section(
    aspace: &mut AddressSpace,
    frame_alloc: &mut dyn FrameAllocator,
    ph: &ProgramHeader,
    flags: mmu::Flags,
    phys_base: PhysicalAddress,
    virt_base: VirtualAddress,
    flush: &mut Flush,
) -> crate::Result<()> {
    let virt_start = virt_base.checked_add(ph.virtual_address).unwrap();
    let zero_start = virt_start.checked_add(ph.file_size).unwrap();
    let zero_end = virt_start.checked_add(ph.mem_size).unwrap();

    let data_bytes_before_zero = zero_start.get() & 0xfff;

    log::debug!(
        "handling BSS {:?}, data bytes before {data_bytes_before_zero}",
        zero_start..zero_end
    );

    if data_bytes_before_zero != 0 {
        let last_page = virt_start
            .checked_add(ph.file_size.saturating_sub(1))
            .unwrap()
            .align_down(ph.align);
        let last_frame = phys_base
            .checked_add(ph.offset + ph.file_size - 1)
            .unwrap()
            .align_down(ph.align);

        let new_frame = frame_alloc
            .allocate_contiguous_zeroed(Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).unwrap())
            .ok_or(mmu::Error::NoMemory)?;

        unsafe {
            let src = slice::from_raw_parts(
                aspace.phys_to_virt(last_frame).as_ptr(),
                data_bytes_before_zero,
            );

            let dst = slice::from_raw_parts_mut(
                aspace.phys_to_virt(new_frame).as_mut_ptr(),
                data_bytes_before_zero,
            );

            log::debug!("copying {data_bytes_before_zero} bytes from {src:p} to {dst:p}...");
            ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
        }

        aspace.remap_contiguous(
            last_page,
            new_frame,
            NonZeroUsize::new(PAGE_SIZE).unwrap(),
            flush,
        )?;
    }

    log::trace!("zero_start {zero_start:?} zero_end {zero_end:?}");
    let (additional_virt_base, additional_len) = {
        // zero_start either lies at a page boundary OR somewhere within the first page
        // by aligning up, we move it to the beginning of the *next* page.
        let start = zero_start.checked_align_up(ph.align).unwrap();
        let end = zero_end.checked_align_up(ph.align).unwrap();
        (start, Range::from(start..end).size())
    };

    if additional_len > 0 {
        let additional_phys = NonContiguousFrames::new_zeroed(
            frame_alloc,
            Layout::from_size_align(additional_len, PAGE_SIZE).unwrap(),
            aspace.physical_memory_offset(),
        );

        log::trace!(
            "mapping additional zeros {additional_virt_base:?}..{:?}",
            additional_virt_base.checked_add(additional_len).unwrap()
        );
        aspace.map(additional_virt_base, additional_phys, flags, flush)?;
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
        let relas = unsafe {
            let ptr = elf_file.input.as_ptr().byte_add(rela_info.offset as usize)
                as *const xmas_elf::sections::Rela<P64>;

            slice::from_raw_parts(ptr, rela_info.count as usize)
        };

        // TODO memory fence here

        log::trace!("applying relocations in virtual memory...");
        for rela in relas {
            apply_relocation(rela, virt_base)?;
        }
    }

    Ok(())
}

fn apply_relocation(
    rela: &xmas_elf::sections::Rela<P64>,
    virt_base: VirtualAddress,
) -> crate::Result<()> {
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
            let target = virt_base.checked_add(rela.get_offset() as usize).unwrap();

            // Calculate the value to store at the relocation target.
            let value = virt_base
                .checked_add_signed(rela.get_addend() as isize)
                .unwrap();

            // log::trace!("reloc R_RISCV_RELATIVE offset: {:#x}; addend: {:#x} => target {target:?} value {value:?}", rela.get_offset(), rela.get_addend());
            unsafe {
                target
                    .as_mut_ptr()
                    .cast::<usize>()
                    .write_unaligned(value.get());
            }
        }
        _ => unimplemented!("unsupported relocation type {}", rela.get_type()),
    }

    Ok(())
}

fn handle_relro_segment(
    aspace: &mut AddressSpace,
    ph: &ProgramHeader,
    virt_base: VirtualAddress,
    flush: &mut Flush,
) -> crate::Result<()> {
    let virt = {
        let start = virt_base.checked_add(ph.virtual_address).unwrap();

        start..start.checked_add(ph.mem_size).unwrap()
    };

    let virt_aligned =
        Range::from(virt.start.align_down(PAGE_SIZE)..virt.end.align_down(PAGE_SIZE));

    log::debug!("Marking RELRO segment {virt_aligned:?} as read-only");
    aspace.protect(
        virt_aligned.start,
        NonZeroUsize::new(virt_aligned.size()).unwrap(),
        mmu::Flags::READ,
        flush,
    )?;

    Ok(())
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
                    if prev.is_some() {
                        panic!("Dynamic section contains more than one Rela entry");
                    }
                }
                Tag::RelaSize => {
                    let val = field.get_val().map_err(Error::Elf)?;
                    let prev = rela_size.replace(val);
                    if prev.is_some() {
                        panic!("Dynamic section contains more than one RelaSize entry");
                    }
                }
                Tag::RelaEnt => {
                    let val = field.get_val().map_err(Error::Elf)?;
                    let prev = rela_ent.replace(val);
                    if prev.is_some() {
                        panic!("Dynamic section contains more than one RelaEnt entry");
                    }
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

fn flags_for_segment(ph: &ProgramHeader) -> mmu::Flags {
    let mut out = mmu::Flags::empty();

    if ph.p_flags.is_read() {
        out |= mmu::Flags::READ;
    }

    if ph.p_flags.is_write() {
        out |= mmu::Flags::WRITE;
    }

    if ph.p_flags.is_execute() {
        out |= mmu::Flags::EXECUTE;
    }

    assert!(
        !out.contains(mmu::Flags::WRITE | mmu::Flags::EXECUTE),
        "elf segment (virtual range {:#x}..{:#x}) is marked as write-execute",
        ph.virtual_address,
        ph.virtual_address + ph.mem_size
    );

    out
}
