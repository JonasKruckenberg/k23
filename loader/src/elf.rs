use bitflags::bitflags;
use core::ffi::CStr;
use core::ops::Range;
use core::slice;
use vmm::{PhysicalAddress, VirtualAddress};

const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// 64-bit object.
pub const ELF_CLASS_64: u8 = 2;
/// 2's complement, little endian.
pub const ELF_DATA_2LSB: u8 = 1;

#[derive(Debug)]
pub struct Section {
    pub virt: Range<VirtualAddress>,
    pub phys: Range<PhysicalAddress>,
}

#[derive(Debug)]
pub struct ElfSections {
    pub entry: VirtualAddress,
    pub text: Section,
    pub rodata: Section,
    pub data: Section,
    pub bss: Section,
    pub tdata: Section,
    pub tbss: Section,
}

#[derive(Debug)]
#[repr(C)]
struct ElfHeader64 {
    pub magic: [u8; 4],
    pub class: u8,
    pub data: u8,
    pub version: u8,
    pub os_abi: u8,
    pub abi_version: u8,
    pub padding: [u8; 7],
    pub e_type: u16,
    pub e_machine: u16,
    pub e_version: u32,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

#[derive(Debug)]
#[repr(C)]
struct ElfSectionHeaderEntry64 {
    sh_name: u32,
    sh_type: u32,
    sh_flags: u64,
    sh_addr: u64,
    sh_offset: u64,
    sh_size: u64,
    sh_link: u32,
    sh_info: u32,
    sh_addralign: u64,
    sh_entsize: u64,
}

bitflags! {
    #[derive(Debug)]
    struct ElfSectionHeaderEntryFlags: usize {
        /// Writable
        const SHF_WRITE = 0x1;
        /// Occupies memory during execution
        const SHF_ALLOC = 0x2;
        /// Executable
        const SHF_EXECINSTR = 0x4;
        /// Might be merged
        const SHF_MERGE = 0x10;
        /// Contains null-terminated strings
        const SHF_STRINGS = 0x20;
        /// 'sh_info' contains SHT index
        const SHF_INFO_LINK = 0x40;
        /// Preserve order after combining
        const SHF_LINK_ORDER = 0x80;
        /// Section hold thread-local data
        const SHF_TLS = 0x400;
    }
}

// This function takes a few shortcuts when parsing the elf file:
//      - we assume the elf file is 64-bit since we have no 32-bit support
//      - we assume the elf file has the same endianness as the loader
pub fn parse(buf: &[u8]) -> ElfSections {
    let hdr = unsafe { &*(buf.as_ptr() as *const ElfHeader64) };
    assert_eq!(
        hdr.magic, ELF_MAGIC,
        "expected kernel to be a valid elf file"
    );
    assert_eq!(
        hdr.class, ELF_CLASS_64,
        "expected kernel to be a 64-bits elf file"
    );
    assert_eq!(
        hdr.data, ELF_DATA_2LSB,
        "expected kernel to be a little-endian elf file"
    );

    let shentries = unsafe {
        let start = buf.as_ptr().add(hdr.e_shoff as usize);
        debug_assert!(start < buf.as_ptr_range().end);
        slice::from_raw_parts(
            start.cast::<ElfSectionHeaderEntry64>(),
            hdr.e_shnum as usize,
        )
    };

    let shstrtab = unsafe {
        let entry = &shentries[hdr.e_shstrndx as usize];
        let start = buf.as_ptr().add(entry.sh_offset as usize);
        slice::from_raw_parts(start, entry.sh_size as usize)
    };

    let mut text_section = None;
    let mut rodata_section = None;
    let mut data_section = None;
    let mut bss_section = None;
    let mut tdata_section = None;
    let mut tbss_section = None;

    for entry in shentries {
        let flags = ElfSectionHeaderEntryFlags::from_bits_retain(entry.sh_flags as usize);

        // either SHT_PROGBITS or SHT_NOBITS and
        if (entry.sh_type == 0x1 || entry.sh_type == 0x8)
            && flags.contains(ElfSectionHeaderEntryFlags::SHF_ALLOC)
        {
            let name = CStr::from_bytes_until_nul(&shstrtab[entry.sh_name as usize..]).unwrap();

            let section = Section {
                virt: unsafe {
                    let start = VirtualAddress::new(entry.sh_addr as usize);
                    start..start.add(entry.sh_size as usize)
                },
                phys: unsafe {
                    let start =
                        PhysicalAddress::new(buf.as_ptr() as usize).add(entry.sh_offset as usize);
                    start..start.add(entry.sh_size as usize)
                },
            };

            match name.to_str().unwrap() {
                ".text" => text_section = Some(section),
                ".rodata" => rodata_section = Some(section),
                ".data" => data_section = Some(section),
                ".bss" => bss_section = Some(section),
                ".tdata" => tdata_section = Some(section),
                ".tbss" => tbss_section = Some(section),
                _ => panic!("unknown section name"),
            }
        }
    }

    assert_ne!(hdr.e_entry, 0, "no entry point found for elf");

    ElfSections {
        entry: unsafe { VirtualAddress::new(hdr.e_entry as usize) },
        text: text_section.expect("elf is missing text section"),
        rodata: rodata_section.expect("elf is missing rodata section"),
        data: data_section.expect("elf is missing data section"),
        bss: bss_section.expect("elf is missing bss section"),
        tdata: tdata_section.expect("elf is missing tdata section"),
        tbss: tbss_section.expect("elf is missing tbss section"),
    }
}
