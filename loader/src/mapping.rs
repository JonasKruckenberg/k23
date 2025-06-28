// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::error::Error;
use crate::frame_alloc::FrameAllocator;
use crate::kernel::Kernel;
use crate::machine_info::MachineInfo;
use crate::page_alloc::PageAllocator;
use crate::{SelfRegions, arch};
use bitflags::bitflags;
use core::alloc::Layout;
use core::num::NonZeroUsize;
use core::range::Range;
use core::{cmp, ptr, slice};
use fallible_iterator::FallibleIterator;
use loader_api::TlsTemplate;
use xmas_elf::P64;
use xmas_elf::dynamic::Tag;
use xmas_elf::program::{SegmentData, Type};

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq)]
    pub struct Flags: u8 {
        const READ = 1 << 0;
        const WRITE = 1 << 1;
        const EXECUTE = 1 << 2;
    }
}

pub fn identity_map_self(
    root_pgtable: usize,
    frame_alloc: &mut FrameAllocator,
    self_regions: &SelfRegions,
) -> crate::Result<()> {
    log::trace!(
        "Identity mapping loader executable region {:#x?}...",
        self_regions.executable
    );
    identity_map_range(
        root_pgtable,
        frame_alloc,
        self_regions.executable,
        Flags::READ | Flags::EXECUTE,
    )?;

    log::trace!(
        "Identity mapping loader read-only region {:#x?}...",
        self_regions.read_only
    );
    identity_map_range(
        root_pgtable,
        frame_alloc,
        self_regions.read_only,
        Flags::READ,
    )?;

    log::trace!(
        "Identity mapping loader read-write region {:#x?}...",
        self_regions.read_write
    );
    identity_map_range(
        root_pgtable,
        frame_alloc,
        self_regions.read_write,
        Flags::READ | Flags::WRITE,
    )?;

    Ok(())
}

#[inline]
fn identity_map_range(
    root_pgtable: usize,
    frame_alloc: &mut FrameAllocator,
    phys: Range<usize>,
    flags: Flags,
) -> crate::Result<()> {
    // Align to page boundaries
    let aligned_start = align_down(phys.start, arch::PAGE_SIZE);
    let aligned_end = checked_align_up(phys.end, arch::PAGE_SIZE).unwrap();
    let len = NonZeroUsize::new(aligned_end.checked_sub(aligned_start).unwrap()).unwrap();

    // Safety: Leaving the address space in an invalid state here is fine since on panic we'll
    // abort startup anyway
    unsafe {
        arch::map_contiguous(
            root_pgtable,
            frame_alloc,
            aligned_start,
            aligned_start,
            len,
            flags,
            0, // called before translation into higher half
        )
    }
}

pub fn map_physical_memory(
    root_pgtable: usize,
    frame_alloc: &mut FrameAllocator,
    page_alloc: &mut PageAllocator,
    minfo: &MachineInfo,
) -> crate::Result<(usize, Range<usize>)> {
    let alignment = arch::page_size_for_level(2);

    let phys = minfo.memory_hull();
    let phys = Range::from(
        align_down(phys.start, alignment)..checked_align_up(phys.end, alignment).unwrap(),
    );

    let virt = Range::from(
        arch::KERNEL_ASPACE_BASE.checked_add(phys.start).unwrap()
            ..arch::KERNEL_ASPACE_BASE.checked_add(phys.end).unwrap(),
    );
    let size = NonZeroUsize::new(phys.end.checked_sub(phys.start).unwrap()).unwrap();

    debug_assert!(phys.start % alignment == 0 && phys.end % alignment == 0);
    debug_assert!(virt.start % alignment == 0 && virt.end % alignment == 0);

    log::trace!("Mapping physical memory {phys:#x?} => {virt:#x?}...");
    // Safety: Leaving the address space in an invalid state here is fine since on panic we'll
    // abort startup anyway
    unsafe {
        arch::map_contiguous(
            root_pgtable,
            frame_alloc,
            virt.start,
            phys.start,
            size,
            Flags::READ | Flags::WRITE,
            0, // called before translation into higher half
        )?;
    }

    // exclude the physical memory map region from page allocation
    page_alloc.reserve(virt.start, size.get());

    Ok((arch::KERNEL_ASPACE_BASE, virt))
}

pub fn map_kernel(
    root_pgtable: usize,
    frame_alloc: &mut FrameAllocator,
    page_alloc: &mut PageAllocator,
    kernel: &Kernel,
    minfo: &MachineInfo,
    phys_off: usize,
) -> crate::Result<(Range<usize>, Option<TlsAllocation>)> {
    let kernel_virt = page_alloc.allocate(
        Layout::from_size_align(
            usize::try_from(kernel.mem_size())?,
            usize::try_from(kernel.max_align())?,
        )
        .unwrap(),
    );

    let phys_base = kernel.elf_file.input.as_ptr() as usize - arch::KERNEL_ASPACE_BASE;
    assert!(
        phys_base % arch::PAGE_SIZE == 0,
        "Loaded ELF file is not sufficiently aligned"
    );

    let mut maybe_tls_allocation = None;

    // Load the segments into virtual memory.
    for ph in kernel.elf_file.program_iter() {
        match ph.get_type().unwrap() {
            Type::Load => handle_load_segment(
                root_pgtable,
                frame_alloc,
                &ProgramHeader::try_from(ph)?,
                phys_base,
                kernel_virt.start,
                phys_off,
            )?,
            Type::Tls => {
                let ph = ProgramHeader::try_from(ph)?;
                let old = maybe_tls_allocation.replace(handle_tls_segment(
                    root_pgtable,
                    frame_alloc,
                    page_alloc,
                    &ph,
                    kernel_virt.start,
                    minfo,
                    phys_off,
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
fn handle_load_segment(
    root_pgtable: usize,
    frame_alloc: &mut FrameAllocator,
    ph: &ProgramHeader,
    phys_base: usize,
    virt_base: usize,
    phys_off: usize,
) -> crate::Result<()> {
    let flags = flags_for_segment(ph);

    log::trace!(
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

        Range::from(align_down(start, ph.align)..checked_align_up(end, ph.align).unwrap())
    };

    let virt = {
        let start = virt_base.checked_add(ph.virtual_address).unwrap();
        let end = start.checked_add(ph.file_size).unwrap();

        Range::from(align_down(start, ph.align)..checked_align_up(end, ph.align).unwrap())
    };

    log::trace!("mapping {virt:#x?} => {phys:#x?}");
    // Safety: Leaving the address space in an invalid state here is fine since on panic we'll
    // abort startup anyway
    unsafe {
        arch::map_contiguous(
            root_pgtable,
            frame_alloc,
            virt.start,
            phys.start,
            NonZeroUsize::new(phys.end.checked_sub(phys.start).unwrap()).unwrap(),
            flags,
            arch::KERNEL_ASPACE_BASE,
        )?;
    }

    if ph.file_size < ph.mem_size {
        handle_bss_section(
            root_pgtable,
            frame_alloc,
            ph,
            flags,
            phys_base,
            virt_base,
            phys_off,
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
fn handle_bss_section(
    root_pgtable: usize,
    frame_alloc: &mut FrameAllocator,
    ph: &ProgramHeader,
    flags: Flags,
    phys_base: usize,
    virt_base: usize,
    phys_off: usize,
) -> crate::Result<()> {
    let virt_start = virt_base.checked_add(ph.virtual_address).unwrap();
    let zero_start = virt_start.checked_add(ph.file_size).unwrap();
    let zero_end = virt_start.checked_add(ph.mem_size).unwrap();

    let data_bytes_before_zero = zero_start & 0xfff;

    log::trace!(
        "handling BSS {:#x?}, data bytes before {data_bytes_before_zero}",
        zero_start..zero_end
    );

    if data_bytes_before_zero != 0 {
        let last_page = align_down(
            virt_start
                .checked_add(ph.file_size.saturating_sub(1))
                .unwrap(),
            ph.align,
        );
        let last_frame = align_down(
            phys_base.checked_add(ph.offset + ph.file_size - 1).unwrap(),
            ph.align,
        );

        let new_frame = frame_alloc.allocate_one_zeroed(arch::KERNEL_ASPACE_BASE)?;

        // Safety: we just allocated the frame
        unsafe {
            let src = slice::from_raw_parts(
                arch::KERNEL_ASPACE_BASE.checked_add(last_frame).unwrap() as *mut u8,
                data_bytes_before_zero,
            );

            let dst = slice::from_raw_parts_mut(
                arch::KERNEL_ASPACE_BASE.checked_add(new_frame).unwrap() as *mut u8,
                data_bytes_before_zero,
            );

            log::trace!("copying {data_bytes_before_zero} bytes from {src:p} to {dst:p}...");
            ptr::copy_nonoverlapping(src.as_ptr(), dst.as_mut_ptr(), dst.len());
        }

        // Safety: Leaving the address space in an invalid state here is fine since on panic we'll
        // abort startup anyway
        unsafe {
            arch::remap_contiguous(
                root_pgtable,
                last_page,
                new_frame,
                NonZeroUsize::new(arch::PAGE_SIZE).unwrap(),
                phys_off,
            );
        }
    }

    log::trace!("zero_start {zero_start:#x} zero_end {zero_end:#x}");
    let (mut virt, len) = {
        // zero_start either lies at a page boundary OR somewhere within the first page
        // by aligning up, we move it to the beginning of the *next* page.
        let start = checked_align_up(zero_start, ph.align).unwrap();
        let end = checked_align_up(zero_end, ph.align).unwrap();
        (start, end.checked_sub(start).unwrap())
    };

    if len > 0 {
        let mut phys_iter = frame_alloc.allocate_zeroed(
            Layout::from_size_align(len, arch::PAGE_SIZE).unwrap(),
            arch::KERNEL_ASPACE_BASE,
        );

        while let Some((phys, len)) = phys_iter.next()? {
            log::trace!(
                "mapping additional zeros {virt:#x}..{:#x}",
                virt.checked_add(len.get()).unwrap()
            );

            // Safety: Leaving the address space in an invalid state here is fine since on panic we'll
            // abort startup anyway
            unsafe {
                arch::map_contiguous(
                    root_pgtable,
                    phys_iter.alloc(),
                    virt,
                    phys,
                    len,
                    flags,
                    arch::KERNEL_ASPACE_BASE,
                )?;
            }

            virt += len.get();
        }
    }

    Ok(())
}

fn handle_dynamic_segment(
    ph: &ProgramHeader,
    elf_file: &xmas_elf::ElfFile,
    virt_base: usize,
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

fn apply_relocation(rela: &xmas_elf::sections::Rela<P64>, virt_base: usize) {
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
            let target = virt_base
                .checked_add(usize::try_from(rela.get_offset()).unwrap())
                .unwrap();

            // Calculate the value to store at the relocation target.
            let value = virt_base
                .checked_add_signed(isize::try_from(rela.get_addend()).unwrap())
                .unwrap();

            // log::trace!("reloc R_RISCV_RELATIVE offset: {:#x}; addend: {:#x} => target {target:?} value {value:?}", rela.get_offset(), rela.get_addend());
            // Safety: we have to trust the ELF data here
            unsafe {
                (target as *mut usize).write_unaligned(value);
            }
        }
        _ => unimplemented!("unsupported relocation type {}", rela.get_type()),
    }
}

/// Map the kernel thread-local storage (TLS) memory regions.
fn handle_tls_segment(
    root_pgtable: usize,
    frame_alloc: &mut FrameAllocator,
    page_alloc: &mut PageAllocator,
    ph: &ProgramHeader,
    virt_base: usize,
    minfo: &MachineInfo,
    phys_off: usize,
) -> crate::Result<TlsAllocation> {
    let layout = Layout::from_size_align(ph.mem_size, cmp::max(ph.align, arch::PAGE_SIZE))
        .unwrap()
        .repeat(minfo.hart_mask.count_ones() as usize)
        .unwrap()
        .0
        .pad_to_align();
    log::trace!("allocating TLS segment {layout:?}...");

    let virt = page_alloc.allocate(layout);
    let mut virt_start = virt.start;

    let mut phys_iter = frame_alloc.allocate_zeroed(layout, phys_off);
    while let Some((phys, len)) = phys_iter.next()? {
        log::trace!(
            "Mapping TLS region {virt_start:#x}..{:#x} {len} ...",
            virt_start.checked_add(len.get()).unwrap()
        );

        // Safety: Leaving the address space in an invalid state here is fine since on panic we'll
        // abort startup anyway
        unsafe {
            arch::map_contiguous(
                root_pgtable,
                phys_iter.alloc(),
                virt_start,
                phys,
                len,
                Flags::READ | Flags::WRITE,
                phys_off,
            )?;
        }

        virt_start += len.get();
    }

    Ok(TlsAllocation {
        virt,
        template: TlsTemplate {
            start_addr: virt_base + ph.virtual_address,
            mem_size: ph.mem_size,
            file_size: ph.file_size,
            align: ph.align,
        },
    })
}

#[derive(Debug)]
pub struct TlsAllocation {
    /// The TLS region in virtual memory
    virt: Range<usize>,
    /// The template we allocated for
    pub template: TlsTemplate,
}

impl TlsAllocation {
    pub fn region_for_hart(&self, hartid: usize) -> Range<usize> {
        let aligned_size = checked_align_up(
            self.template.mem_size,
            cmp::max(self.template.align, arch::PAGE_SIZE),
        )
        .unwrap();
        let start = self.virt.start + (aligned_size * hartid);

        Range::from(start..start + self.template.mem_size)
    }

    pub fn initialize_for_hart(&self, hartid: usize) {
        if self.template.file_size != 0 {
            // Safety: We have to trust the loaders BootInfo here
            unsafe {
                let src: &[u8] = slice::from_raw_parts(
                    self.template.start_addr as *const u8,
                    self.template.file_size,
                );
                let dst: &mut [u8] = slice::from_raw_parts_mut(
                    self.region_for_hart(hartid).start as *mut u8,
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

pub fn map_kernel_stacks(
    root_pgtable: usize,
    frame_alloc: &mut FrameAllocator,
    page_alloc: &mut PageAllocator,
    minfo: &MachineInfo,
    per_cpu_size_pages: usize,
    phys_off: usize,
) -> crate::Result<StacksAllocation> {
    let per_cpu_size = per_cpu_size_pages * arch::PAGE_SIZE;
    let per_cpu_size_with_guard = per_cpu_size + arch::PAGE_SIZE;

    let layout_with_guard = Layout::from_size_align(per_cpu_size_with_guard, arch::PAGE_SIZE)
        .unwrap()
        .repeat(minfo.hart_mask.count_ones() as usize)
        .unwrap()
        .0;

    let virt = page_alloc.allocate(layout_with_guard);
    log::trace!("Mapping stacks region {virt:#x?}...");

    for hart in 0..minfo.hart_mask.count_ones() {
        let layout = Layout::from_size_align(per_cpu_size, arch::PAGE_SIZE).unwrap();

        let mut virt = virt
            .end
            .checked_sub(per_cpu_size_with_guard * hart as usize)
            .and_then(|a| a.checked_sub(per_cpu_size))
            .unwrap();

        log::trace!("Allocating stack {layout:?}...");
        // The stacks region doesn't need to be zeroed, since we will be filling it with
        // the canary pattern anyway
        let mut phys_iter = frame_alloc.allocate(layout);

        while let Some((phys, len)) = phys_iter.next()? {
            log::trace!(
                "mapping stack for hart {hart} {virt:#x}..{:#x} => {phys:#x}..{:#x}",
                virt.checked_add(len.get()).unwrap(),
                phys.checked_add(len.get()).unwrap()
            );

            // Safety: Leaving the address space in an invalid state here is fine since on panic we'll
            // abort startup anyway
            unsafe {
                arch::map_contiguous(
                    root_pgtable,
                    phys_iter.alloc(),
                    virt,
                    phys,
                    len,
                    Flags::READ | Flags::WRITE,
                    phys_off,
                )?;
            }

            virt += len.get();
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
    virt: Range<usize>,
    per_cpu_size: usize,
    per_cpu_size_with_guard: usize,
}

impl StacksAllocation {
    pub fn region_for_cpu(&self, cpuid: usize) -> Range<usize> {
        let end = self.virt.end - (self.per_cpu_size_with_guard * cpuid);

        Range::from((end - self.per_cpu_size)..end)
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

fn flags_for_segment(ph: &ProgramHeader) -> Flags {
    let mut out = Flags::empty();

    if ph.p_flags.is_read() {
        out |= Flags::READ;
    }

    if ph.p_flags.is_write() {
        out |= Flags::WRITE;
    }

    if ph.p_flags.is_execute() {
        out |= Flags::EXECUTE;
    }

    assert!(
        !out.contains(Flags::WRITE | Flags::EXECUTE),
        "elf segment (virtual range {:#x}..{:#x}) is marked as write-execute",
        ph.virtual_address,
        ph.virtual_address + ph.mem_size
    );

    out
}

#[must_use]
#[inline]
pub const fn checked_align_up(this: usize, align: usize) -> Option<usize> {
    assert!(
        align.is_power_of_two(),
        "checked_align_up: align is not a power-of-two"
    );

    // SAFETY: `align` has been checked to be a power of 2 above
    let align_minus_one = unsafe { align.unchecked_sub(1) };

    // addr.wrapping_add(align_minus_one) & 0usize.wrapping_sub(align)
    if let Some(addr_plus_align) = this.checked_add(align_minus_one) {
        let aligned = addr_plus_align & 0usize.wrapping_sub(align);
        debug_assert!(aligned % align == 0);
        debug_assert!(aligned >= this);
        Some(aligned)
    } else {
        None
    }
}

#[must_use]
#[inline]
pub const fn align_down(this: usize, align: usize) -> usize {
    assert!(
        align.is_power_of_two(),
        "align_down: align is not a power-of-two"
    );

    let aligned = this & 0usize.wrapping_sub(align);
    debug_assert!(aligned % align == 0);
    debug_assert!(aligned <= this);
    aligned
}
