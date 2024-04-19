use bitflags::bitflags;
use core::ffi::CStr;
use core::ops::Range;
use vmm::{PhysicalAddress, VirtualAddress};

#[derive(Debug, Clone)]
pub struct Section {
    pub virt: Range<VirtualAddress>,
    pub phys: Range<PhysicalAddress>,
}

#[derive(Debug, Clone)]
pub struct ElfSections {
    pub entry: VirtualAddress,
    pub text: Section,
    pub rodata: Section,
    pub data: Section,
    pub bss: Section,
    pub tls: Section,
}

#[derive(Debug)]
#[repr(C)]
struct ElfHeader {
    ei_magic: [u8; 4],
    ei_class: u8,
    ei_data: u8,
    ei_version: u8,
    ei_osabi: u8,
    ei_abiversion: u8,
    ei_pad: [u8; 7],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: usize,
    e_phoff: usize,
    e_shoff: usize,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

#[derive(Debug)]
#[repr(C)]
struct ElfSectionHeaderEntry {
    sh_name: u32,
    sh_type: u32,
    sh_flags: usize,
    sh_addr: usize,
    sh_offset: usize,
    sh_size: usize,
    sh_link: u32,
    sh_info: u32,
    sh_addralign: usize,
    sh_entsize: usize,
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

pub fn parse(buf: &[u8]) -> ElfSections {
    let hdr = unsafe { &*(buf.as_ptr() as *const ElfHeader) };

    let shentries = unsafe {
        let start = buf.as_ptr().add(hdr.e_shoff) as *const ElfSectionHeaderEntry;
        core::slice::from_raw_parts(start, hdr.e_shnum as usize)
    };

    let shstrtab = unsafe {
        let entry = &shentries[hdr.e_shstrndx as usize];

        let start = buf.as_ptr().add(entry.sh_offset);
        core::slice::from_raw_parts(start, entry.sh_size)
    };

    let mut text_section = None;
    let mut rodata_section = None;
    let mut data_section = None;
    let mut bss_section = None;
    let mut tls_section = None;

    for entry in shentries {
        let flags = ElfSectionHeaderEntryFlags::from_bits_retain(entry.sh_flags);

        // either SHT_PROGBITS or SHT_NOBITS and
        if (entry.sh_type == 0x1 || entry.sh_type == 0x8)
            && flags.contains(ElfSectionHeaderEntryFlags::SHF_ALLOC)
        {
            let name = CStr::from_bytes_until_nul(&shstrtab[entry.sh_name as usize..]).unwrap();

            let section = Section {
                virt: unsafe {
                    let start = VirtualAddress::new(entry.sh_addr);
                    start..start.add(entry.sh_size)
                },
                phys: unsafe {
                    let start = PhysicalAddress::new(buf.as_ptr() as usize).add(entry.sh_offset);
                    start..start.add(entry.sh_size)
                },
            };

            match name.to_str().unwrap() {
                ".text" => text_section = Some(section),
                ".rodata" => rodata_section = Some(section),
                ".data" => data_section = Some(section),
                ".bss" => bss_section = Some(section),
                ".tls" => tls_section = Some(section),
                _ => panic!("unknown section name"),
            }
        }
    }

    assert_ne!(hdr.e_entry, 0, "no entry point found for elf");

    ElfSections {
        entry: unsafe { VirtualAddress::new(hdr.e_entry) },
        text: text_section.expect("elf is missing text section"),
        rodata: rodata_section.expect("elf is missing rodata section"),
        data: data_section.expect("elf is missing data section"),
        bss: bss_section.expect("elf is missing bss section"),
        tls: tls_section.expect("elf is missing bss section"),
    }
}
