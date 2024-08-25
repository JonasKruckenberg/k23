use crate::{AddressRangeExt, Flush, Mapper, Mode, PhysicalAddress, VirtualAddress};
use core::ops::Div;
use core::{ptr, slice};
use object::elf::{
    ProgramHeader64, Rela64, DT_RELA, DT_RELACOUNT, DT_RELAENT, DT_RELASZ, PT_DYNAMIC,
    PT_GNU_RELRO, PT_LOAD, PT_TLS, R_RISCV_RELATIVE,
};
use object::read::elf::{Dyn, ElfFile64, ElfSectionIterator64, ProgramHeader as _, Rela};
use object::{Endianness, Object, ObjectSection};

impl<'a, M: Mode> Mapper<'a, M> {
    pub fn elf(&mut self, virtual_base: VirtualAddress) -> ElfMapper<'_, 'a, M> {
        ElfMapper {
            inner: self,
            virtual_base,
        }
    }
}

pub struct ElfMapper<'p, 'a, M> {
    inner: &'p mut Mapper<'a, M>,
    virtual_base: VirtualAddress,
}

impl<'p, 'a, M: Mode> ElfMapper<'p, 'a, M> {
    /// Maps an ELF file into virtual memory.
    ///
    /// # Errors
    ///
    /// Returns an error if the ELF file could not be mapped, due to various reasons like, malformed ELF, failing allocations, etc.
    ///
    /// # Panics
    ///
    /// Panics on various sanity checks.
    pub fn map_elf_file(
        &mut self,
        elf_file: &ElfFile64,
        flush: &mut Flush<M>,
    ) -> crate::Result<Option<TlsTemplate>> {
        let physical_base = PhysicalAddress::new(elf_file.data().as_ptr() as usize);
        assert!(
            physical_base.is_aligned(M::PAGE_SIZE),
            "Loaded ELF file is not sufficiently aligned"
        );

        let mut tls_template = None;

        let program_headers = elf_file
            .elf_program_headers()
            .iter()
            .filter_map(|h| ProgramHeader::try_from(h).ok());

        // Load the segments into virtual memory.
        for program_header in program_headers.clone() {
            match program_header.p_type {
                PT_LOAD => self.handle_load_segment(&program_header, physical_base, flush)?,
                PT_TLS => {
                    let old = tls_template.replace(self.handle_tls_segment(&program_header));
                    assert!(old.is_none(), "multiple TLS segments not supported");
                }
                _ => {}
            }
        }

        // Apply relocations in virtual memory.
        for program_header in elf_file.elf_program_headers() {
            if program_header.p_type.get(Endianness::Little) == PT_DYNAMIC {
                self.handle_dynamic_segment(&program_header, physical_base, elf_file)?;
            }
        }

        // Mark some memory regions as read-only after relocations have been
        // applied.
        for program_header in program_headers {
            if program_header.p_type == PT_GNU_RELRO {
                self.handle_relro_segment(&program_header, flush)?;
            }
        }

        Ok(tls_template)
    }

    fn handle_load_segment(
        &mut self,
        ph: &ProgramHeader,
        phys_base: PhysicalAddress,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        let flags = flags_for_segment::<M>(ph);

        log::info!(
            "Handling Segment: LOAD off {offset:#016x} vaddr {vaddr:#016x} align {align} filesz {filesz:#016x} memsz {memsz:#016x} flags {flags:?}",
            offset = ph.offset,
            vaddr = ph.virtual_address,
            align = ph.align,
            filesz = ph.file_size,
            memsz = ph.mem_size
        );

        let phys = {
            let start = phys_base.add(ph.offset);
            let end = start.add(ph.file_size);

            start.align_down(ph.align)..end.align_up(ph.align)
        };

        let virt = {
            let start = self.virtual_base.add(ph.virtual_address);
            let end = start.add(ph.file_size);

            start.align_down(ph.align)..end.align_up(ph.align)
        };

        log::trace!("mapping {virt:?} => {phys:?}");
        self.inner.map_range(virt, phys, flags, flush)?;

        self.inner
            .map_range(virt_aligned, phys_aligned, flags, flush)?;

        if program_header.file_size < program_header.mem_size {
            self.handle_bss_section(program_header, flags, physical_base, flush)?;
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
    ///     2.3. lastly we replace last page previously mapped by `handle_load_segment` to stitch things up.
    /// 3. If the BSS section is larger than that one page, we allocate additional zeroed frames and map them in.
    fn handle_bss_section(
        &mut self,
        program_header: &ProgramHeader,
        flags: M::EntryFlags,
        physical_base: PhysicalAddress,
        flush: &mut Flush<M>,
    ) -> Result<(), crate::Error> {
        // calculate virtual memory region that must be zeroed
        let virt_start = self.virtual_base.add(program_header.virtual_address);
        let zero_start = virt_start.add(program_header.file_size);
        let zero_end = virt_start.add(program_header.mem_size);

        let data_bytes_before_zero = zero_start.as_raw() & 0xfff;

        log::debug!(
            "handling BSS {:?}, data before {data_bytes_before_zero}",
            zero_start..zero_end
        );
        if data_bytes_before_zero != 0 {
            let last_page = virt_start
                .add(program_header.file_size - 1)
                .align_down(M::PAGE_SIZE);
            let last_frame = physical_base
                .add(program_header.offset + program_header.file_size - 1)
                .align_down(M::PAGE_SIZE);

            let new_frame = self.allocate_and_copy(last_frame, data_bytes_before_zero)?;

            log::debug!(
                "remap {:?} to {:?}",
                last_page..last_page.add(M::PAGE_SIZE),
                new_frame.add(M::PAGE_SIZE)
            );

            self.inner.remap(last_page, new_frame, flags, flush)?;
        }

        let additional_virt = {
            let start = zero_start.align_up(M::PAGE_SIZE).align_down(M::PAGE_SIZE);
            let end = zero_end.align_up(M::PAGE_SIZE);
            start..end
        };

        if !additional_virt.is_empty() {
            // additional_virt should be page-aligned, but just to make sure
            debug_assert!(additional_virt.is_aligned(M::PAGE_SIZE));

            let additional_phys = {
                let start = self
                    .inner
                    .allocator_mut()
                    .allocate_frames_zeroed(additional_virt.size().div(M::PAGE_SIZE))?;

                start..start.add(additional_virt.size())
            };

            log::trace!("mapping additional zeros {additional_virt:?} => {additional_phys:?}");
            self.inner
                .map_range(additional_virt, additional_phys, flags, flush)?;
        }

        Ok(())
    }

    #[allow(clippy::unused_self)]
    fn handle_tls_segment(&mut self, program_header: &ProgramHeader) -> TlsTemplate {
        TlsTemplate {
            start_addr: self.virtual_base.add(program_header.virtual_address),
            mem_size: program_header.mem_size,
            file_size: program_header.file_size,
        }
    }

    fn handle_dynamic_segment(
        &mut self,
        program_header: &ProgramHeader64<Endianness>,
        physical_base: PhysicalAddress,
        elf_file: &ElfFile64,
    ) -> Result<(), crate::Error> {
        let data = program_header
            .dynamic(Endianness::Little, elf_file.data())
            .unwrap()
            .unwrap();

        let mut rela = None; // Address of Rela relocs
        let mut rela_size = None; // Total size of Rela relocs
        let mut rela_ent = None; // Size of one Rela reloc
        let mut rela_count = None; // Number of Rela relocs

        for rel in data {
            let tag = rel.tag32(Endianness::Little).unwrap();
            match tag {
                DT_RELA => {
                    let ptr = rel.d_val(Endianness::Little);
                    let prev = rela.replace(ptr);
                    if prev.is_some() {
                        panic!("Dynamic section contains more than one Rela entry");
                    }
                }
                DT_RELASZ => {
                    let val = rel.d_val(Endianness::Little);
                    let prev = rela_size.replace(val);
                    if prev.is_some() {
                        panic!("Dynamic section contains more than one RelaSize entry");
                    }
                }
                DT_RELAENT => {
                    let val = rel.d_val(Endianness::Little);
                    let prev = rela_ent.replace(val);
                    if prev.is_some() {
                        panic!("Dynamic section contains more than one RelaEnt entry");
                    }
                }
                DT_RELACOUNT => {
                    let val = rel.d_val(Endianness::Little);
                    let prev = rela_count.replace(val);
                    if prev.is_some() {
                        panic!("Dynamic section contains more than one RelaCount entry");
                    }
                }
                _ => {}
            }
        }

        log::debug!(
            "rela address {rela:x?} total rela size {rela_size:?} rela entry size {rela_ent:?}"
        );

        let offset = if let Some(rela) = rela {
            rela
        } else {
            // The section doesn't contain any relocations.

            if rela_size.is_some() || rela_ent.is_some() {
                panic!("Rela entry is missing but RelaSize or RelaEnt have been provided");
            }

            return Ok(());
        };
        let total_size = rela_size.expect("RelaSize entry is missing");
        let entry_size = rela_ent.expect("RelaEnt entry is missing");
        let rela_count = rela_count.expect("RelaCount entry is missing");

        assert_eq!(
            entry_size,
            size_of::<Rela64<Endianness>>() as u64,
            "unsupported entry size: {entry_size}"
        );
        assert_eq!(total_size / entry_size, rela_count, "invalid RelaCount");

        let relas = unsafe {
            let ptr = physical_base.add(offset as usize).as_raw() as *const Rela64<Endianness>;

            slice::from_raw_parts(ptr, rela_count as usize)
        };

        for rela in relas {
            self.apply_relocation(rela, physical_base, elf_file)?;
        }

        Ok(())
    }

    fn apply_relocation(
        &mut self,
        rela: &Rela64<Endianness>,
        physical_base: PhysicalAddress,
        elf_file: &ElfFile64,
    ) -> Result<(), crate::Error> {
        assert!(
            rela.symbol(Endianness::Little, false).is_none(),
            "relocations using the symbol table are not supported"
        );

        match rela.r_type(Endianness::Little, false) {
            R_RISCV_RELATIVE => {
                // Calculate the relocation target
                let offset = rela.r_offset(Endianness::Little);
                let target = physical_base.add(offset as usize);

                // Calculate the relocated value.
                let addend = rela.r_addend(Endianness::Little) as isize;
                let value = self.virtual_base.offset(addend);

                // let section = sections.find_map(|sec| {
                //     let (start, size) = sec.file_range()?;
                //     if (start..start + size).contains(&rela.r_offset(Endianness::Little)) {
                //         Some(sec.name().ok()?)
                //     } else {
                //         None
                //     }
                // });

                log::trace!(
                    "Resoling relocation R_RISCV_RELATIVE at {offset:#x} = {:?} + {addend:#x} ({:?}) section {section:?}",
                    self.virtual_base,
                    value,
                );

                unsafe { (target.as_raw() as *mut usize).write_unaligned(value.as_raw()) };
            }
            ty => unimplemented!("unsupported relocation type {:?}", ty),
        }

        Ok(())
    }

    fn handle_relro_segment(
        &mut self,
        program_header: &ProgramHeader,
        flush: &mut Flush<M>,
    ) -> Result<(), crate::Error> {
        let virt = {
            let start = self.virtual_base.add(program_header.virtual_address);

            start..start.add(program_header.mem_size)
        };

        let virt_aligned =
            { virt.start.align_down(M::PAGE_SIZE)..virt.end.align_down(M::PAGE_SIZE) };

        log::debug!("Marking RELRO segment {virt_aligned:?} as read-only");
        self.inner
            .set_flags_for_range(virt_aligned, M::ENTRY_FLAGS_RO, flush)?;

        Ok(())
    }

    fn allocate_and_copy(
        &mut self,
        src: PhysicalAddress,
        len: usize,
    ) -> Result<PhysicalAddress, crate::Error> {
        let frames = len.div_ceil(M::PAGE_SIZE);
        let dst = self.inner.allocator_mut().allocate_frames(frames)?;

        unsafe {
            let src = slice::from_raw_parts_mut(src.as_raw() as *mut u8, len);

            let dst = slice::from_raw_parts_mut(dst.as_raw() as *mut u8, len);

            log::debug!("copy {len} bytes from {src:p} to {dst:p}");

            ptr::copy_nonoverlapping(src.as_mut_ptr(), dst.as_mut_ptr(), dst.len());
        }

        Ok(dst)
    }

fn flags_for_segment<M: Mode>(program_header: &ProgramHeader) -> M::EntryFlags {
    if program_header.p_flags & 0x1 != 0 {
        M::ENTRY_FLAGS_RX
    } else if program_header.p_flags & 0x2 != 0 {
        M::ENTRY_FLAGS_RW
    } else if program_header.p_flags & 0x4 != 0 {
        M::ENTRY_FLAGS_RO
    } else {
        panic!("invalid segment flags {:?}", program_header.p_flags)
    }
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct TlsTemplate {
    /// The address of TLS template
    pub start_addr: VirtualAddress,
    /// The size of the TLS segment in memory
    pub mem_size: usize,
    /// The size of the TLS segment in the elf file.
    /// If the TLS segment contains zero-initialized data (tbss) then this size will be smaller than
    /// `mem_size`
    pub file_size: usize,
}

struct ProgramHeader {
    pub p_type: u32,
    pub p_flags: u32,
    pub offset: usize,
    pub virtual_address: usize,
    pub file_size: usize,
    pub mem_size: usize,
}

impl TryFrom<&ProgramHeader64<Endianness>> for ProgramHeader {
    type Error = &'static str;

    fn try_from(value: &ProgramHeader64<Endianness>) -> Result<Self, Self::Error> {
        let endianness = Endianness::default();

        Ok(Self {
            p_type: value.p_type(endianness),
            p_flags: value.p_flags(endianness),
            offset: usize::try_from(value.p_offset(endianness))
                .map_err(|_| "failed to convert p_offset to usize")?,
            virtual_address: usize::try_from(value.p_vaddr(endianness))
                .map_err(|_| "failed to convert p_vaddr to usize")?,
            file_size: usize::try_from(value.p_filesz(endianness))
                .map_err(|_| "failed to convert p_filesz to usize")?,
            mem_size: usize::try_from(value.p_memsz(endianness))
                .map_err(|_| "failed to convert p_memsz to usize")?,
        })
    }
}
