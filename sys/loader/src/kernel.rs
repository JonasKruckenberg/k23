// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::range::Range;
use core::slice;

use arrayvec::ArrayVec;
use loader_api::TlsTemplate;
use mem_core::{PhysicalAddress, VirtualAddress};
use object::LittleEndian;
use object::elf::{
    DT_REL, DT_RELA, DT_RELAENT, DT_RELASZ, DT_RELENT, DT_RELR, DT_RELRENT, DT_RELRSZ, DT_RELSZ,
    Dyn64, FileHeader64, PF_R, PF_W, PF_X, PT_DYNAMIC, PT_GNU_RELRO, PT_LOAD, PT_TLS,
    ProgramHeader64, R_RISCV_RELATIVE, Rela64,
};
use object::read::elf::{Dyn, FileHeader, ProgramHeader, Rela};
use uefi::boot::{AllocateType, MemoryType};
use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode, RegularFile};
use uefi::{CStr16, Status, cstr16};

use crate::error::Error;

const KERNEL_PATH: &CStr16 = cstr16!("EFI\\k23\\kernel.elf");
const KERNEL_DEBUGINFO_PATH: &CStr16 = cstr16!("EFI\\k23\\kernel.debug");

// 64 segments ought to be enough for anyone!
const MAX_LOAD_SEGMENTS: usize = 64;

/// Locate the kernel payload of disk.
/// When this completes successfully we have found the kernel payload
/// files and opened readable handles to it.
pub fn locate() -> crate::Result<(RegularFile, Option<RegularFile>)> {
    let mut fs = uefi::boot::get_image_file_system(uefi::boot::image_handle())?;
    let mut root = fs.open_volume()?;

    let kernel = root.open(KERNEL_PATH, FileMode::Read, FileAttribute::empty())?;
    let kernel = kernel.into_regular_file().unwrap();

    let debug_info = match root
        .open(
            KERNEL_DEBUGINFO_PATH,
            FileMode::Read,
            FileAttribute::empty(),
        )
        .map_err(uefi::Error::split)
    {
        Ok(debug_info) => Some(debug_info.into_regular_file().unwrap()),
        Err((Status::NOT_FOUND, _)) => None,
        Err((status, data)) => return Err(Error::from(uefi::Error::new(status, data))),
    };

    Ok((kernel, debug_info))
}

pub struct KernelImage {
    /// The size of the kernel in memory
    size: usize,
    /// The entrypoint as reported by the ELF, offset relative to the in-memory image
    pub entry: usize,
    pub load: ArrayVec<LoadSegment, MAX_LOAD_SEGMENTS>,
    tls: TlsTemplate,
    dynamic: ProgramHeader64<LittleEndian>,
    /// The range (offsets into in-memory image) that must be marked as read-only after appling relocations.
    relro: Range<usize>,
}

pub struct Kernel {
    file: RegularFile,
    image: KernelImage,
    debug_info: Option<RegularFile>,
}

impl Kernel {
    /// Parse the located kernel payload.
    /// When this completes successfully we have validated the file structure
    /// and parsed out relevant information.
    pub fn from_files(
        mut kernel: RegularFile,
        debug_info: Option<RegularFile>,
    ) -> crate::Result<Self> {
        // Read the ELF header + program-header table into a heap page rather than
        // a stack buffer: a PAGE_SIZE stack array plus the firmware's deep
        // file-read call chain can overrun the modest UEFI stack into adjacent
        // firmware structures. The page is `LOADER_DATA`, reclaimed at handoff.
        let buf = {
            let base =
                uefi::boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1)?;
            // Safety: one freshly-allocated page, identity-mapped while boot services
            // are live; never freed, so the borrow is valid for the rest of `init`.
            unsafe { slice::from_raw_parts_mut(base.as_ptr(), uefi::boot::PAGE_SIZE) }
        };
        kernel.set_position(0)?; // make sure we're at the beginning
        let bytes_read = kernel.read(buf)?;
        assert_eq!(bytes_read, buf.len(), "kernel ELF too short"); // TODO turn into error

        let header = FileHeader64::<LittleEndian>::parse(&*buf)?;
        log::debug!("kernel header {header:?}");

        // TODO assert arch matches current
        // TODO assert bitness matches current
        // TODO assert endianness matches current

        #[cfg(debug_assertions)]
        {
            let phtable_end = header.e_phoff(LittleEndian)
                + u64::from(header.e_phnum(LittleEndian))
                    * u64::from(header.e_phentsize(LittleEndian));
            assert!(phtable_end <= buf.len() as u64);
        }

        let mut load = ArrayVec::new();
        let mut dynamic = None;
        let mut tls = None;
        let mut relro = None;

        for ph in header.program_headers(LittleEndian, &*buf)? {
            match ph.p_type(LittleEndian) {
                PT_LOAD => {
                    load.try_push(parse_load_segment(ph)?).unwrap(); // TODO error
                }
                PT_TLS => {
                    assert!(tls.replace(parse_tls_segment(ph)?).is_none()) // TODO error
                }
                PT_DYNAMIC => assert!(dynamic.replace(ph.clone()).is_none()), // TODO error
                PT_GNU_RELRO => {
                    let start = usize::try_from(ph.p_vaddr(LittleEndian)).unwrap();
                    let len = usize::try_from(ph.p_memsz(LittleEndian)).unwrap();
                    let range = Range::from(start..start + len);

                    assert!(relro.replace(range).is_none())
                } // TODO error
                _ => continue,
            }
        }

        assert!(load.is_sorted_by_key(|seg| seg.image_offset)); // TODO error
        assert!(load.first().unwrap().image_offset == 0); // TODO error

        let size = {
            let last_seg = load.last().unwrap();
            let image_end = last_seg
                .image_offset
                .checked_add(last_seg.mem_size)
                .unwrap(); // TODO error

            image_end
                .checked_next_multiple_of(uefi::boot::PAGE_SIZE)
                .unwrap()
        };

        log::trace!("kernel image size: {size} bytes");

        Ok(Kernel {
            file: kernel,
            debug_info,
            image: KernelImage {
                size,
                entry: usize::try_from(header.e_entry(LittleEndian)).unwrap(),
                load,
                tls: tls.unwrap(),         // TODO error
                dynamic: dynamic.unwrap(), // TODO error
                relro: relro.unwrap(),     // TODO error
            },
        })
    }

    /// Stage the kernel into physical memory.
    /// When this completes successfully we have allocated physical memory and
    /// initialized it (from disk or zeroed as appropriate).
    pub fn stage(mut self) -> crate::Result<StagedKernel> {
        fn allocate(size: usize) -> crate::Result<&'static mut [u8]> {
            let pages = size.div_ceil(uefi::boot::PAGE_SIZE);

            log::trace!("allocating {pages} physmem pages");

            // NB: allocate as memory type "RESERVED" so we don't classify it as reclaimable on handoff
            let base =
                uefi::boot::allocate_pages(AllocateType::AnyPages, MemoryType::RESERVED, pages)?;

            Ok(unsafe { slice::from_raw_parts_mut(base.as_ptr(), size) })
        }

        let staging = allocate(self.image.size)?;

        for seg in &self.image.load {
            // NB: explain the filesz<->memsz difference, BSS and how if the BSS ends before
            // the page boundary we potentially end up with uninitialized memory being mapped
            // which is of course not a great idea from a security POV. We therefore just zero
            // everything up to the next page boundary.
            let span = seg.unaligned_mem_range();
            let seg_end = span
                .end
                .checked_next_multiple_of(uefi::boot::PAGE_SIZE)
                .unwrap(); // TODO error

            log::trace!("attempting to read {:?}", span.start..seg_end);
            let dst = staging.get_mut(span.start..seg_end).unwrap();

            if seg.file_size > 0 {
                self.file.set_position(seg.file_offset)?;

                let bytes_read = self.file.read(&mut dst[..seg.file_size])?;
                debug_assert_eq!(bytes_read, seg.file_size); // TODO error
            }

            dst[seg.file_size..].fill(0); // BSS + over-zero to page boundary
        }

        let debug_info = if let Some(mut debug_info) = self.debug_info {
            let info = debug_info.get_boxed_info::<FileInfo>()?;

            let buf = allocate(info.file_size() as usize)?;
            let bytes_read = debug_info.read(buf)?;
            assert_eq!(bytes_read, buf.len());

            Some(buf)
        } else {
            None
        };

        Ok(StagedKernel {
            staging,
            debug_info,
            image: self.image,
        })
    }
}

pub struct StagedKernel {
    staging: &'static mut [u8],
    debug_info: Option<&'static mut [u8]>,
    image: KernelImage,
}

struct RelaInfo {
    offset: usize,
    count: usize,
}

impl StagedKernel {
    pub fn size(&self) -> usize {
        self.image.size
    }

    pub fn tls_template(&self) -> &TlsTemplate {
        &self.image.tls
    }

    /// Apply relocations to the staged kernel.
    /// When this completes successfully the kernel images is ready for execution
    /// _within_ its address space.
    pub fn relocate(self, kernel_virt: Range<VirtualAddress>) -> crate::Result<RelocatedKernel> {
        let rela_info = self
            .parse_rela()?
            .expect("k23 kernel MUST have relocations"); // TODO error

        for i in 0..rela_info.count {
            let off = rela_info.offset + i * size_of::<Rela64<LittleEndian>>();
            let (rela, _) =
                object::pod::from_bytes::<Rela64<LittleEndian>>(&self.staging[off..]).unwrap();

            match rela.r_type(LittleEndian, false) {
                R_RISCV_RELATIVE => {
                    // dynamic relocation offsets are relative to the virtual
                    // layout of the elf, not the physical file
                    let target = usize::try_from(rela.r_offset(LittleEndian)).unwrap(); // TODO error
                    let value = kernel_virt
                        .start
                        .offset(isize::try_from(rela.r_addend(LittleEndian)).unwrap()); // TODO error
                    assert!(kernel_virt.contains(&value)); // TODO error

                    self.staging[target..target + 8].copy_from_slice(&value.get().to_le_bytes());
                }
                ty => unimplemented!("unsupported relocation type {ty}"),
            }
        }

        Ok(RelocatedKernel {
            staging: self.staging,
            debug_info: self.debug_info,
            image: self.image,
            virt: kernel_virt,
        })
    }

    fn parse_rela(&self) -> crate::Result<Option<RelaInfo>> {
        // NB: not using `object`s `.dynamic` method here because we access the segment through the staging buffer
        // which requires memory-offsets, not file-offsets like `.dynamic` uses.
        let fields = {
            let vaddr = usize::try_from(self.image.dynamic.p_vaddr(LittleEndian)).unwrap(); // TODO error
            let filesz = usize::try_from(self.image.dynamic.p_filesz(LittleEndian)).unwrap(); // TODO error

            object::pod::slice_from_all_bytes::<Dyn64<LittleEndian>>(
                &self.staging[vaddr..vaddr + filesz],
            )
            .unwrap() // TODO error
        };

        let mut rela = None; // Address of Rela relocs
        let mut rela_size = None; // Total size of Rela relocs
        let mut rela_ent = None; // Size of one Rela reloc

        for field in fields {
            let property = match field.tag(LittleEndian) {
                DT_RELA => &mut rela,
                DT_RELASZ => &mut rela_size,
                DT_RELAENT => &mut rela_ent,
                DT_REL | DT_RELSZ | DT_RELENT => {
                    unimplemented!("REL relocations are not supported")
                }
                DT_RELR | DT_RELRSZ | DT_RELRENT => {
                    unimplemented!("RELR relocations are not supported")
                }
                _ => continue,
            };

            let val = usize::try_from(field.val(LittleEndian)).unwrap(); // TODO error
            let prev = property.replace(val);
            assert!(prev.is_none()); // TODO error
        }

        #[expect(clippy::manual_assert, reason = "cleaner this way")]
        if rela.is_none() && (rela_size.is_some() || rela_ent.is_some()) {
            panic!("Rela entry is missing but RelaSize or RelaEnt have been provided");
        }

        // No RELA field means no relocations at all
        let Some(offset) = rela else {
            return Ok(None);
        };

        let total_size = rela_size.expect("RelaSize entry is missing");
        let entry_size = rela_ent.expect("RelaEnt entry is missing");

        assert_eq!(entry_size, size_of::<Rela64<LittleEndian>>()); // TODO error

        Ok(Some(RelaInfo {
            offset,
            count: total_size / entry_size,
        }))
    }
}

pub struct RelocatedKernel {
    staging: &'static mut [u8],
    debug_info: Option<&'static mut [u8]>,
    image: KernelImage,
    virt: Range<VirtualAddress>,
}

impl RelocatedKernel {
    /// Physical base address of the staged kernel image.
    ///
    /// Valid only while UEFI boot services are active: before `ExitBootServices`
    /// the staging buffer is identity-mapped, so the pointer's address *is* its
    /// physical address.
    pub fn phys_base(&self) -> PhysicalAddress {
        PhysicalAddress::new(self.staging.as_ptr().addr())
    }

    pub fn entry(&self) -> VirtualAddress {
        let entry = self.virt.start.add(self.image.entry);
        debug_assert!(self.virt.contains(&entry));
        entry
    }

    pub fn debug_info_phys(&self) -> Option<Range<PhysicalAddress>> {
        let range = self.debug_info.as_ref()?.as_ptr_range();
        Some(Range::from(
            PhysicalAddress::from_ptr(range.start)..PhysicalAddress::from_ptr(range.end),
        ))
    }

    pub fn load_segments(&self) -> impl ExactSizeIterator<Item = &'_ LoadSegment> {
        self.image.load.iter()
    }

    pub fn tls_template(&self) -> &TlsTemplate {
        &self.image.tls
    }

    pub fn relro_range(&self) -> Range<usize> {
        self.image.relro
    }

    /// Allocate the boot hart's TLS block and copy the `.tdata` template into it.
    ///
    /// This copies out of the staging buffer rather than the file, and must run
    /// *after* relocation: `.tdata` can itself carry `R_RISCV_RELATIVE`
    /// relocations, and those are only applied to the staging buffer. Copying
    /// from disk would yield an unrelocated — and therefore corrupt — template.
    pub fn instantiate_tls_block(&self) -> crate::Result<&'static mut [u8]> {
        let seg = &self.image.tls;

        let tls_size = seg.mem_size.next_multiple_of(uefi::boot::PAGE_SIZE);
        let tls_pages = tls_size / uefi::boot::PAGE_SIZE;
        debug_assert!(tls_pages > 0);

        log::trace!("allocating {tls_pages} physmem pages for boot hart TLS block");
        let block =
            uefi::boot::allocate_pages(AllocateType::AnyPages, MemoryType::RESERVED, tls_pages)?;

        // Safety: `allocate_pages` returned `tls_pages` pages (`tls_size` bytes) of
        // freshly reserved memory; it is never freed, so `'static` is sound.
        let block = unsafe { slice::from_raw_parts_mut(block.as_ptr(), tls_size) };

        // `.tdata` lives at `image_offset` in the staged (and now relocated) image.
        let tdata = &self.staging[seg.image_offset..seg.image_offset + seg.file_size];
        block[..seg.file_size].copy_from_slice(tdata);
        block[seg.file_size..].fill(0); // .tbss + over-zero to page boundary

        Ok(block)
    }
}

/// Access permissions of a `PT_LOAD` segment, decoded from the ELF `p_flags`.
#[derive(Debug, Clone, Copy)]
pub enum Permissions {
    ReadOnly,
    ReadWrite,
    ReadExecute,
}

pub struct LoadSegment {
    /// Offset into the in-memory image where this segment starts
    pub image_offset: usize,
    /// Size of the segment in memory
    pub mem_size: usize,
    /// Offset into the on-disk file where this segment starts
    pub file_offset: u64,
    /// Size of the segment on disk
    pub file_size: usize,
    pub perms: Permissions,
}

impl LoadSegment {
    /// Byte range this segment occupies in the in-memory image — the file-backed
    /// bytes followed by the zero-initialized BSS tail.
    pub fn unaligned_mem_range(&self) -> Range<usize> {
        Range::from(self.image_offset..self.image_offset + self.mem_size)
    }
}

fn parse_load_segment(ph: &ProgramHeader64<LittleEndian>) -> crate::Result<LoadSegment> {
    assert!(usize::try_from(ph.p_align(LittleEndian)).unwrap() == uefi::boot::PAGE_SIZE); // TODO error

    let image_offset = usize::try_from(ph.p_vaddr(LittleEndian)).unwrap(); // TODO error
    // assert!(
    //     image_offset.is_multiple_of(uefi::boot::PAGE_SIZE),
    //     "{image_offset} is not page aligned"
    // ); // TODO error

    let file_offset = ph.p_offset(LittleEndian);

    let file_size = usize::try_from(ph.p_filesz(LittleEndian)).unwrap(); // TODO error
    let mem_size = usize::try_from(ph.p_memsz(LittleEndian)).unwrap(); // TODO error

    const RW: u32 = PF_R | PF_W;
    const RX: u32 = PF_R | PF_X;
    let flags = ph.p_flags(LittleEndian);
    let perms = match flags {
        PF_R => Permissions::ReadOnly,
        RW => Permissions::ReadWrite,
        RX => Permissions::ReadExecute,
        _ => return Err(Error::InvalidSegmentFlags(flags)),
    };

    Ok(LoadSegment {
        image_offset,
        mem_size,
        file_offset,
        file_size,
        perms,
    })
}

fn parse_tls_segment(ph: &ProgramHeader64<LittleEndian>) -> crate::Result<TlsTemplate> {
    let image_offset = usize::try_from(ph.p_vaddr(LittleEndian)).unwrap(); // TODO error

    let file_size = usize::try_from(ph.p_filesz(LittleEndian)).unwrap(); // TODO error
    let mem_size = usize::try_from(ph.p_memsz(LittleEndian)).unwrap(); // TODO error

    let align = usize::try_from(ph.p_align(LittleEndian)).unwrap(); // TODO error

    Ok(TlsTemplate {
        image_offset,
        mem_size,
        file_size,
        align,
    })
}
