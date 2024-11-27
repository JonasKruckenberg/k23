use crate::{AddressRangeExt, PhysicalAddress, VirtualAddress};
use core::ops::Div;
use core::{ptr, slice};
use kmm::{Flush, Mapper, Mode};
use xmas_elf::dynamic::Tag;
use xmas_elf::program::{SegmentData, Type};
use xmas_elf::P64;
use loader_api::TlsTemplate;

pub struct ElfMapper<'p, 'a, M> {
    inner: &'p mut Mapper<'a, M>,
    virtual_base: VirtualAddress,
}

impl<'p, 'a, M: Mode> ElfMapper<'p, 'a, M> {
    pub fn new(mapper: &'p mut Mapper<'a, M>, virtual_base: VirtualAddress) -> Self {
        Self {
            inner: mapper,
            virtual_base,
        }
    }

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
        elf_file: &xmas_elf::ElfFile,
        flush: &mut Flush<M>,
    ) -> crate::Result<Option<TlsTemplate>> {
        let physical_base = PhysicalAddress::new(elf_file.input.as_ptr() as usize);
        assert!(
            physical_base.is_aligned(M::PAGE_SIZE),
            "Loaded ELF file is not sufficiently aligned"
        );

        let mut tls_template = None;

        // Load the segments into virtual memory.
        for ph in elf_file.program_iter() {
            match ph.get_type().unwrap() {
                Type::Load => self.handle_load_segment(
                    &ProgramHeader::try_from(ph).unwrap(),
                    physical_base,
                    flush,
                )?,
                Type::Tls => {
                    let old = tls_template
                        .replace(self.handle_tls_segment(&ProgramHeader::try_from(ph).unwrap()));
                    assert!(old.is_none(), "multiple TLS segments not supported");
                }
                _ => {}
            }
        }

        // Apply relocations in virtual memory.
        for ph in elf_file.program_iter() {
            if ph.get_type().unwrap() == Type::Dynamic {
                self.handle_dynamic_segment(
                    &ProgramHeader::try_from(ph).unwrap(),
                    physical_base,
                    elf_file,
                )?;
            }
        }

        // Mark some memory regions as read-only after relocations have been
        // applied.
        for ph in elf_file.program_iter() {
            if ph.get_type().unwrap() == Type::GnuRelro {
                self.handle_relro_segment(&ProgramHeader::try_from(ph).unwrap(), flush)?;
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

        if ph.file_size < ph.mem_size {
            self.handle_bss_section(ph, flags, phys_base, flush)?;
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
        ph: &ProgramHeader,
        flags: M::EntryFlags,
        phys_base: PhysicalAddress,
        flush: &mut Flush<M>,
    ) -> crate::Result<()> {
        let virt_start = self.virtual_base.add(ph.virtual_address);
        let zero_start = virt_start.add(ph.file_size);
        let zero_end = virt_start.add(ph.mem_size);

        let data_bytes_before_zero = zero_start.as_raw() & 0xfff;

        log::debug!(
            "handling BSS {:?}, data bytes before {data_bytes_before_zero}",
            zero_start..zero_end
        );

        if data_bytes_before_zero != 0 {
            let last_page = virt_start.add(ph.file_size - 1).align_down(ph.align);
            let last_frame = phys_base
                .add(ph.offset + ph.file_size - 1)
                .align_down(ph.align);

            let new_frame = self.allocate_and_copy(last_frame, data_bytes_before_zero)?;

            log::debug!(
                "remapping {:?} to {:?}",
                last_page..last_page.add(ph.align),
                new_frame..new_frame.add(ph.align)
            );

            self.inner.remap(last_page, new_frame, flags, flush)?;
        }

        let additional_virt = {
            let start = zero_start.align_up(ph.align).align_down(ph.align);
            let end = zero_end.align_up(ph.align);
            start..end
        };

        if !additional_virt.is_empty() {
            // additional_virt should be page-aligned, but just to make sure
            debug_assert!(additional_virt.is_aligned(ph.align));

            let additional_phys = {
                let start = self
                    .inner
                    .allocator_mut()
                    .allocate_frames_zeroed(additional_virt.size().div(ph.align))?;

                start..start.add(additional_virt.size())
            };

            log::trace!("mapping additional zeros {additional_virt:?} => {additional_phys:?}");
            self.inner
                .map_range(additional_virt, additional_phys, flags, flush)?;
        }

        Ok(())
    }

    #[allow(clippy::unused_self)]
    fn handle_tls_segment(&mut self, ph: &ProgramHeader) -> TlsTemplate {
        TlsTemplate {
            start_addr: self.virtual_base.add(ph.virtual_address),
            mem_size: ph.mem_size,
            file_size: ph.file_size,
        }
    }

    fn handle_dynamic_segment(
        &mut self,
        ph: &ProgramHeader,
        phys_base: PhysicalAddress,
        elf_file: &xmas_elf::ElfFile,
    ) -> crate::Result<()> {
        if let Some(rela_info) = ph.parse_rela(elf_file)? {
            let relas = unsafe {
                let ptr = phys_base.add(rela_info.offset as usize).as_raw()
                    as *const xmas_elf::sections::Rela<P64>;

                slice::from_raw_parts(ptr, rela_info.count as usize)
            };

            for rela in relas {
                self.apply_relocation(rela)?;
            }
        }

        Ok(())
    }

    fn apply_relocation(&mut self, rela: &xmas_elf::sections::Rela<P64>) -> crate::Result<()> {
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
                let target = self.virtual_base.add(rela.get_offset() as usize);

                // Calculate the value to store at the relocation target.
                let value = self.virtual_base.offset(rela.get_addend() as isize);

                let target_phys = self
                    .inner
                    .virt_to_phys(target)
                    .expect("relocation target not mapped");

                unsafe { (target_phys.as_raw() as *mut usize).write_unaligned(value.as_raw()) };
            }
            _ => unimplemented!("unsupported relocation type {}", rela.get_type()),
        }

        Ok(())
    }

    fn handle_relro_segment(
        &mut self,
        ph: &ProgramHeader,
        flush: &mut Flush<M>,
    ) -> Result<(), crate::Error> {
        let virt = {
            let start = self.virtual_base.add(ph.virtual_address);

            start..start.add(ph.mem_size)
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
}

fn flags_for_segment<M: Mode>(ph: &ProgramHeader) -> M::EntryFlags {
    if ph.p_flags.is_execute() {
        M::ENTRY_FLAGS_RX
    } else if ph.p_flags.is_write() {
        M::ENTRY_FLAGS_RW
    } else if ph.p_flags.is_read() {
        M::ENTRY_FLAGS_RO
    } else {
        panic!("invalid segment flags {:?}", ph.p_flags)
    }
}

pub struct ProgramHeader<'a> {
    pub p_flags: xmas_elf::program::Flags,
    pub align: usize,
    pub offset: usize,
    pub virtual_address: usize,
    pub file_size: usize,
    pub mem_size: usize,
    ph: xmas_elf::program::ProgramHeader<'a>,
}

impl ProgramHeader<'_> {
    fn parse_rela(&self, elf_file: &xmas_elf::ElfFile) -> crate::Result<Option<RelaInfo>> {
        let data = self.ph.get_data(elf_file).map_err(crate::Error::Elf)?;
        let fields = match data {
            SegmentData::Dynamic32(_) => unimplemented!("32-bit elf files are not supported"),
            SegmentData::Dynamic64(fields) => fields,
            _ => return Ok(None),
        };

        let mut rela = None; // Address of Rela relocs
        let mut rela_size = None; // Total size of Rela relocs
        let mut rela_ent = None; // Size of one Rela reloc

        for field in fields {
            let tag = field.get_tag().map_err(crate::Error::Elf)?;
            match tag {
                Tag::Rela => {
                    let ptr = field.get_ptr().map_err(crate::Error::Elf)?;
                    let prev = rela.replace(ptr);
                    if prev.is_some() {
                        panic!("Dynamic section contains more than one Rela entry");
                    }
                }
                Tag::RelaSize => {
                    let val = field.get_val().map_err(crate::Error::Elf)?;
                    let prev = rela_size.replace(val);
                    if prev.is_some() {
                        panic!("Dynamic section contains more than one RelaSize entry");
                    }
                }
                Tag::RelaEnt => {
                    let val = field.get_val().map_err(crate::Error::Elf)?;
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
    type Error = crate::Error;

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
