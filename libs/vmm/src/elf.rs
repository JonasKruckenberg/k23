use crate::{Flush, Mapper, Mode, PhysicalAddress, VirtualAddress};
use core::{ptr, slice};
use object::elf::{ProgramHeader64, PT_DYNAMIC, PT_GNU_RELRO, PT_LOAD, PT_TLS};
use object::read::elf::{ElfFile64, ProgramHeader as _};
use object::Endianness;

struct ProgramHeader {
    pub p_type: u32,
    pub p_flags: u32,
    pub offset: usize,
    pub virtual_address: VirtualAddress,
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
            virtual_address: {
                let raw = usize::try_from(value.p_vaddr(endianness))
                    .map_err(|_| "failed to convert p_vaddr to usize")?;

                if raw == 0 {
                    return Err("p_vaddr is zero");
                }

                VirtualAddress::new(raw)
            },
            file_size: usize::try_from(value.p_filesz(endianness))
                .map_err(|_| "failed to convert p_filesz to usize")?,
            mem_size: usize::try_from(value.p_memsz(endianness))
                .map_err(|_| "failed to convert p_memsz to usize")?,
        })
    }
}

impl<'a, M: Mode> Mapper<'a, M> {
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
        let physical_offset = PhysicalAddress::new(elf_file.data().as_ptr() as usize);
        assert!(
            physical_offset.is_aligned(M::PAGE_SIZE),
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
                PT_LOAD => self.handle_load_segment(&program_header, physical_offset, flush)?,
                PT_TLS => {
                    let old = tls_template.replace(self.handle_tls_segment(&program_header));
                    assert!(old.is_none(), "multiple TLS segments not supported");
                }
                _ => {}
            }
        }

        // Apply relocations in virtual memory.
        for program_header in program_headers.clone() {
            if program_header.p_type == PT_DYNAMIC {
                self.handle_dynamic_segment(&program_header)?;
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
        program_header: &ProgramHeader,
        physical_offset: PhysicalAddress,
        flush: &mut Flush<M>,
    ) -> Result<(), crate::Error> {
        let phys_aligned = {
            let start = physical_offset
                .add(program_header.offset)
                .align_down(M::PAGE_SIZE);
            let end = start.add(program_header.file_size).align_up(M::PAGE_SIZE);

            start..end
        };

        let virt_aligned = {
            let start = program_header.virtual_address.align_down(M::PAGE_SIZE);

            let end = start.add(program_header.file_size).align_up(M::PAGE_SIZE);

            start..end
        };

        let flags = Self::flags_for_segment(program_header);

        log::trace!("{flags:?}");
        log::trace!(
            "segment {:#x?} => {:#x?}",
            program_header.virtual_address
                ..program_header.virtual_address.add(program_header.mem_size),
            program_header.offset..program_header.offset + program_header.file_size
        );
        log::trace!("mapping {virt_aligned:?} => {phys_aligned:?}");

        self.map_range(virt_aligned, phys_aligned, flags, flush)?;

        if program_header.file_size < program_header.mem_size {
            self.handle_bss_section(program_header, flags, physical_offset, flush)?;
        }

        Ok(())
    }

    fn handle_bss_section(
        &mut self,
        program_header: &ProgramHeader,
        flags: M::EntryFlags,
        physical_offset: PhysicalAddress,
        flush: &mut Flush<M>,
    ) -> Result<(), crate::Error> {
        // calculate virtual memory region that must be zeroed
        let virt_start = program_header.virtual_address;
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
            let last_frame = physical_offset
                .add(program_header.offset + program_header.file_size - 1)
                .align_down(M::PAGE_SIZE);

            let new_frame = self.allocate_and_copy(last_frame, data_bytes_before_zero)?;

            log::debug!("remap {:?} to {:?}", last_page, new_frame);

            self.remap(last_page, new_frame, flags, flush)?;
        }

        let additional = {
            let start = zero_start.align_up(M::PAGE_SIZE).align_down(M::PAGE_SIZE);
            let end = zero_end.align_down(M::PAGE_SIZE);
            start..end
        };

        assert!(additional.is_empty());

        // TODO for BSS pages that are left unaccounted for after this allocate frames & map

        Ok(())
    }

    #[allow(clippy::unused_self)]
    fn handle_tls_segment(&mut self, program_header: &ProgramHeader) -> TlsTemplate {
        TlsTemplate {
            start_addr: program_header.virtual_address,
            mem_size: program_header.mem_size,
            file_size: program_header.file_size,
        }
    }

    fn handle_dynamic_segment(
        &mut self,
        _program_header: &ProgramHeader,
    ) -> Result<(), crate::Error> {
        todo!()
    }

    fn handle_relro_segment(
        &mut self,
        program_header: &ProgramHeader,
        flush: &mut Flush<M>,
    ) -> Result<(), crate::Error> {
        let virt = {
            let start = program_header.virtual_address;

            start..start.add(program_header.mem_size)
        };

        let virt_aligned =
            { virt.start.align_down(M::PAGE_SIZE)..virt.end.align_down(M::PAGE_SIZE) };

        log::debug!("Marking RELRO segment {virt_aligned:?} as read-only");
        self.set_flags_for_range(virt_aligned, M::ENTRY_FLAGS_RO, flush)?;

        Ok(())
    }

    fn allocate_and_copy(
        &mut self,
        src: PhysicalAddress,
        len: usize,
    ) -> Result<PhysicalAddress, crate::Error> {
        let frames = len.div_ceil(M::PAGE_SIZE);
        let dst = self.allocator_mut().allocate_frames(frames)?;

        unsafe {
            let src = slice::from_raw_parts_mut(src.as_raw() as *mut u8, len);

            let dst = slice::from_raw_parts_mut(dst.as_raw() as *mut u8, len);

            log::debug!("copy {len} bytes from {src:p} to {dst:p}");

            ptr::copy_nonoverlapping(src.as_mut_ptr(), dst.as_mut_ptr(), dst.len());
        }

        Ok(dst)
    }

    fn flags_for_segment(program_header: &ProgramHeader) -> M::EntryFlags {
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
