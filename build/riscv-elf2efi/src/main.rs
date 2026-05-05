// Adapted from gimli-rs/object's elftoefi example:
// https://github.com/gimli-rs/object/blob/main/crates/examples/src/bin/elftoefi.rs
//
// Forked rather than depended on because (a) it's an example binary, not a
// published crate, and (b) we needed RISC-V-specific fixes / k23 adaptations
// (notably the GOT_HI20 + PCREL_LO12_I pairing pass and trimming the tool to
// only the architectures k23 cares about).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::Parser;
use object::read::elf::{FileHeader, Rela, SectionHeader, SectionTable};
use object::{Endianness, ReadRef, SectionIndex, elf, pe};

#[derive(Parser, Debug)]
#[command(about = "Convert an ELF executable into an EFI one")]
struct Args {
    /// Input ELF path.
    #[arg(short, long)]
    input: PathBuf,

    /// Output EFI path.
    #[arg(short, long)]
    output: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let input = fs::File::open(&args.input)
        .with_context(|| format!("Failed to open file '{}'", args.input.display()))?;

    let in_data = unsafe {
        memmap2::Mmap::map(&input)
            .with_context(|| format!("Failed to map file '{}'", args.input.display()))?
    };

    let in_data = &*in_data;

    let kind = object::FileKind::parse(in_data).context("Failed to parse file")?;

    let out_data = match kind {
        object::FileKind::Elf32 => copy_file::<elf::FileHeader32<Endianness>>(in_data)?,
        object::FileKind::Elf64 => copy_file::<elf::FileHeader64<Endianness>>(in_data)?,
        _ => anyhow::bail!("Not an ELF file"),
    };

    fs::write(&args.output, out_data)
        .with_context(|| format!("Failed to write file '{}'", args.output.display()))?;

    Ok(())
}

// UEFI requires PE section alignment of at least 4 KiB (the firmware uses it to
// apply page-level protection). Individual ELF section sh_addralign values are
// typically smaller, so we pin the PE section/file alignment to a page.
const PE_ALIGNMENT: u64 = 0x1000;

struct SectionLayout {
    text_start: u32,
    text_end: u32,
    have_data: bool,
    data_start: u32,
    data_end: u32,
    // Extent of initialized (PROGBITS) data — the PE data section's file size
    // excludes trailing BSS.
    data_file_end: u32,
}

fn scan_layout<Elf: FileHeader<Endian = Endianness>>(
    in_sections: &SectionTable<'_, Elf>,
    endian: Endianness,
) -> anyhow::Result<SectionLayout> {
    let mut text_start: u32 = !0;
    let mut text_end: u32 = 0;
    let mut have_data = false;
    let mut data_start: u32 = !0;
    let mut data_end: u32 = 0;
    let mut data_file_end: u32 = 0;

    for in_section in in_sections.iter() {
        if !is_alloc(in_section, endian) {
            continue;
        }
        assert!(in_section.sh_addralign(endian).into() <= PE_ALIGNMENT);
        let start: u32 = in_section.sh_addr(endian).into().try_into()?;
        let size: u32 = in_section.sh_size(endian).into().try_into()?;
        let end: u32 = start
            .checked_add(size)
            .context("section end overflows u32")?;
        if is_text(in_section, endian) {
            assert!(text_end <= start);
            text_start = text_start.min(start);
            text_end = text_end.max(end);
        } else if is_data(in_section, endian) {
            assert!(data_end <= start);
            have_data = true;
            data_start = data_start.min(start);
            data_end = data_end.max(end);
            if in_section.sh_type(endian) == elf::SHT_PROGBITS {
                data_file_end = data_file_end.max(end);
            }
        } else {
            unreachable!();
        }
    }
    assert!(text_start <= text_end);
    if have_data {
        assert!(text_end <= data_start);
        assert!(data_start <= data_end);
    }

    Ok(SectionLayout {
        text_start,
        text_end,
        have_data,
        data_start,
        data_end,
        data_file_end,
    })
}

struct Relocations {
    pe_relocs: Vec<(u32, u16)>,
    // For absolute RELA relocations we need to patch the target slot with the
    // addend when copying section bytes — otherwise UEFI's PE loader will
    // "relocate" a zeroed slot and every pointer lands at the image base.
    rela_addends: HashMap<u32, (i64, u16)>,
}

fn collect_relocations<Elf: FileHeader<Endian = Endianness>>(
    in_sections: &SectionTable<'_, Elf>,
    in_data: &[u8],
    endian: Endianness,
) -> anyhow::Result<Relocations> {
    let mut pe_relocs: Vec<(u32, u16)> = Vec::new();
    let mut rela_addends: HashMap<u32, (i64, u16)> = HashMap::new();

    for in_section in in_sections.iter() {
        if in_section.rel(endian, in_data)?.is_some() {
            // RISC-V uses RELA exclusively; an SHT_REL section here means an
            // unexpected toolchain output.
            bail!("unexpected REL section in RISC-V ELF");
        }
        let Some((relas, _)) = in_section.rela(endian, in_data)? else {
            continue;
        };

        // Static rela sections have sh_info pointing at the target section.
        // Dynamic rela sections (.rela.dyn, .rela.plt) are SHF_ALLOC and
        // target virtual addresses across the whole image.
        let dynamic = (in_section.sh_flags(endian).into() as u32) & elf::SHF_ALLOC != 0;
        let (info_addr, info_data): (u64, &[u8]) = if dynamic {
            (0, &[])
        } else {
            let info_index = SectionIndex(in_section.sh_info(endian) as usize);
            let info = in_sections.section(info_index)?;
            if !is_alloc(info, endian) {
                continue;
            }
            (info.sh_addr(endian).into(), info.data(endian, in_data)?)
        };

        // R_RISCV_GOT_HI20 sets got_address; the immediately following
        // R_RISCV_PCREL_LO12_I consumes it. Scoped per-section to catch
        // unpaired relocations.
        let mut got_address: Option<u32> = None;
        let mut got_addresses: Vec<u32> = Vec::new();
        for rela in relas {
            let r_offset: u32 = rela.r_offset(endian).into().try_into()?;
            let r_type = rela.r_type(endian, false);
            match r_type {
                elf::R_RISCV_BRANCH
                | elf::R_RISCV_JAL
                | elf::R_RISCV_CALL
                | elf::R_RISCV_CALL_PLT
                | elf::R_RISCV_RVC_BRANCH
                | elf::R_RISCV_RVC_JUMP
                | elf::R_RISCV_PCREL_HI20 => {}
                elf::R_RISCV_ADD32 | elf::R_RISCV_SUB32 => {}
                elf::R_RISCV_GOT_HI20 => {
                    let info_offset = u64::from(r_offset).wrapping_sub(info_addr);
                    let instruction = info_data
                        .read_at::<object::U32<Elf::Endian>>(info_offset)
                        .ok()
                        .context("R_RISCV_GOT_HI20: instruction read out of bounds")?
                        .get(endian);
                    assert_eq!(instruction & 0x7f, 0x17);
                    got_address = Some(r_offset.wrapping_add(instruction & 0xffff_f000));
                }
                elf::R_RISCV_PCREL_LO12_I => {
                    if let Some(mut addr) = got_address.take() {
                        let info_offset = u64::from(r_offset).wrapping_sub(info_addr);
                        let instruction = info_data
                            .read_at::<object::U32<Elf::Endian>>(info_offset)
                            .ok()
                            .context("R_RISCV_PCREL_LO12_I: instruction read out of bounds")?
                            .get(endian);
                        assert_eq!(instruction & 0x707f, 0x3003);
                        addr = addr.wrapping_add(((instruction & 0xfff0_0000) as i32 >> 20) as u32);
                        got_addresses.push(addr);
                    }
                }
                elf::R_RISCV_64 | elf::R_RISCV_RELATIVE | elf::R_RISCV_JUMP_SLOT => {
                    pe_relocs.push((r_offset, pe::IMAGE_REL_BASED_DIR64));
                    rela_addends.insert(
                        r_offset,
                        (rela.r_addend(endian).into(), pe::IMAGE_REL_BASED_DIR64),
                    );
                }
                elf::R_RISCV_32 => {
                    pe_relocs.push((r_offset, pe::IMAGE_REL_BASED_HIGHLOW));
                    rela_addends.insert(
                        r_offset,
                        (rela.r_addend(endian).into(), pe::IMAGE_REL_BASED_HIGHLOW),
                    );
                }
                _ => bail!("unsupported RISC-V relocation at offset {r_offset:#x}, type {r_type}"),
            }
        }
        if got_address.is_some() {
            bail!("unpaired R_RISCV_GOT_HI20");
        }

        got_addresses.sort_unstable();
        got_addresses.dedup();
        for addr in got_addresses {
            pe_relocs.push((addr, pe::IMAGE_REL_BASED_DIR64));
        }
    }

    pe_relocs.sort_unstable();
    pe_relocs.dedup();

    Ok(Relocations {
        pe_relocs,
        rela_addends,
    })
}

fn copy_file<Elf: FileHeader<Endian = Endianness>>(in_data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let in_elf = Elf::parse(in_data)?;
    let endian = in_elf.endian()?;
    let in_sections = in_elf.sections(endian, in_data)?;

    let layout = scan_layout::<Elf>(&in_sections, endian)?;

    let machine = match in_elf.e_machine(endian) {
        elf::EM_RISCV => {
            if in_elf.is_class_64() {
                pe::IMAGE_FILE_MACHINE_RISCV64
            } else {
                pe::IMAGE_FILE_MACHINE_RISCV32
            }
        }
        other => bail!("unsupported ELF e_machine: {other} (riscv-elf2efi only handles EM_RISCV)"),
    };

    let relocs = collect_relocations::<Elf>(&in_sections, in_data, endian)?;

    // Reserve file ranges and virtual addresses.
    let mut out_data = Vec::new();
    let mut writer = object::write::pe::Writer::new(
        in_elf.is_type_64(),
        PE_ALIGNMENT as u32,
        PE_ALIGNMENT as u32,
        &mut out_data,
    );

    for &(offset, kind) in &relocs.pe_relocs {
        writer.add_reloc(offset, kind);
    }

    let mut section_num = 1;
    if layout.have_data {
        section_num += 1;
    }
    if writer.has_relocs() {
        section_num += 1;
    }

    writer.reserve_dos_header();
    writer.reserve_nt_headers(16);
    writer.reserve_section_headers(section_num);
    writer.reserve_virtual_until(layout.text_start);
    let text_range = writer.reserve_text_section(layout.text_end - layout.text_start);
    assert_eq!(text_range.virtual_address, layout.text_start);
    let mut data_range = Default::default();
    if layout.have_data {
        writer.reserve_virtual_until(layout.data_start);
        data_range = writer.reserve_data_section(
            layout.data_end - layout.data_start,
            layout.data_file_end.saturating_sub(layout.data_start),
        );
        assert_eq!(data_range.virtual_address, layout.data_start);
    }
    if writer.has_relocs() {
        writer.reserve_reloc_section();
    }

    // Write section bytes, patching absolute RELA targets with their addend.
    // Under `-shared` lld may leave the target slots zeroed and store the
    // addend only in the RELA entry.
    let write_section = |writer: &mut object::write::pe::Writer,
                         in_section: &<Elf as FileHeader>::SectionHeader,
                         section_start: u32,
                         section_file_offset: u32|
     -> anyhow::Result<()> {
        let sh_addr: u32 = in_section.sh_addr(endian).into().try_into()?;
        let sh_size: u32 = in_section.sh_size(endian).into().try_into()?;
        let offset = sh_addr.checked_sub(section_start).unwrap();
        writer.pad_until(section_file_offset + offset);

        let mut bytes = in_section.data(endian, in_data)?.to_vec();
        for (&r_offset, &(addend, reloc_type)) in &relocs.rela_addends {
            if r_offset < sh_addr || r_offset >= sh_addr + sh_size {
                continue;
            }
            let slot = (r_offset - sh_addr) as usize;
            if reloc_type == pe::IMAGE_REL_BASED_DIR64 {
                if slot + 8 > bytes.len() {
                    continue;
                }
                bytes[slot..slot + 8].copy_from_slice(&(addend as u64).to_le_bytes());
            } else {
                if slot + 4 > bytes.len() {
                    continue;
                }
                let addend32 = i32::try_from(addend).with_context(|| {
                    format!("HIGHLOW addend {addend} at offset {r_offset:#x} doesn't fit in i32")
                })?;
                bytes[slot..slot + 4].copy_from_slice(&addend32.to_le_bytes());
            }
        }
        writer.write(&bytes);
        Ok(())
    };

    writer.write_empty_dos_header()?;
    writer.write_nt_headers(object::write::pe::NtHeaders {
        machine,
        time_date_stamp: 0,
        characteristics: if in_elf.is_class_64() {
            pe::IMAGE_FILE_EXECUTABLE_IMAGE
                | pe::IMAGE_FILE_LINE_NUMS_STRIPPED
                | pe::IMAGE_FILE_LOCAL_SYMS_STRIPPED
                | pe::IMAGE_FILE_LARGE_ADDRESS_AWARE
        } else {
            pe::IMAGE_FILE_EXECUTABLE_IMAGE
                | pe::IMAGE_FILE_LINE_NUMS_STRIPPED
                | pe::IMAGE_FILE_LOCAL_SYMS_STRIPPED
                | pe::IMAGE_FILE_32BIT_MACHINE
        },
        major_linker_version: 0,
        minor_linker_version: 0,
        address_of_entry_point: in_elf.e_entry(endian).into().try_into()?,
        image_base: 0,
        major_operating_system_version: 0,
        minor_operating_system_version: 0,
        major_image_version: 0,
        minor_image_version: 0,
        major_subsystem_version: 0,
        minor_subsystem_version: 0,
        subsystem: pe::IMAGE_SUBSYSTEM_EFI_APPLICATION,
        dll_characteristics: 0,
        size_of_stack_reserve: 0,
        size_of_stack_commit: 0,
        size_of_heap_reserve: 0,
        size_of_heap_commit: 0,
    });
    writer.write_section_headers();

    writer.pad_until(text_range.file_offset);
    for in_section in in_sections.iter() {
        if !is_text(in_section, endian) {
            continue;
        }
        write_section(
            &mut writer,
            in_section,
            text_range.virtual_address,
            text_range.file_offset,
        )?;
    }
    writer.pad_until(text_range.file_offset + text_range.file_size);
    if layout.have_data {
        for in_section in in_sections.iter() {
            if !is_data(in_section, endian) {
                continue;
            }
            if in_section.sh_type(endian) == elf::SHT_NOBITS {
                continue;
            }
            write_section(
                &mut writer,
                in_section,
                data_range.virtual_address,
                data_range.file_offset,
            )?;
        }
        writer.pad_until(data_range.file_offset + data_range.file_size);
    }
    writer.write_reloc_section();

    debug_assert_eq!(writer.reserved_len() as usize, writer.len());

    Ok(out_data)
}

// Only PROGBITS/NOBITS sections contribute runtime content. Dynamic-linking
// metadata (SHT_DYNSYM, SHT_HASH, SHT_DYNAMIC, SHT_RELA, ...) is discarded:
// UEFI firmware performs PE-style loading and applies the base relocations we
// emit, so those sections are dead weight in the output image.
fn is_runtime_section<S: SectionHeader>(s: &S, endian: S::Endian) -> bool {
    let sh_type = s.sh_type(endian);
    sh_type == elf::SHT_PROGBITS || sh_type == elf::SHT_NOBITS
}

fn is_text<S: SectionHeader>(s: &S, endian: S::Endian) -> bool {
    let flags = s.sh_flags(endian).into() as u32;
    is_runtime_section(s, endian)
        && flags & elf::SHF_ALLOC != 0
        && (flags & elf::SHF_EXECINSTR != 0 || flags & elf::SHF_WRITE == 0)
}

fn is_data<S: SectionHeader>(s: &S, endian: S::Endian) -> bool {
    let flags = s.sh_flags(endian).into() as u32;
    is_runtime_section(s, endian)
        && flags & elf::SHF_ALLOC != 0
        && flags & elf::SHF_EXECINSTR == 0
        && flags & elf::SHF_WRITE != 0
}

fn is_alloc<S: SectionHeader>(s: &S, endian: S::Endian) -> bool {
    let flags = s.sh_flags(endian).into() as u32;
    is_runtime_section(s, endian) && flags & elf::SHF_ALLOC != 0
}
