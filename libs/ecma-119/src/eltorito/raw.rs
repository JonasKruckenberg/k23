use core::mem::size_of;

use zerocopy::byteorder::{LittleEndian, U16, U32};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use super::{BootPlatform, EmulationType, emulation_from_u8, platform_from_u8};

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct ValidationEntry {
    pub header_id: u8,   // must be 1
    pub platform_id: u8, // 0 = 80x86, 1 = PowerPC, 2 = Mac, 0xef = UEFI
    _reserved: [u8; 2],
    pub id: [u8; 24],
    pub checksum: U16<LittleEndian>,
    pub key: [u8; 2], // must be 0x55, 0xAA
}
const _: () = assert!(size_of::<ValidationEntry>() == 32);

impl ValidationEntry {
    pub fn platform(&self) -> BootPlatform {
        platform_from_u8(self.platform_id)
    }
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct InitialEntry {
    pub boot_indicator: u8,
    pub boot_media_ty: u8, // TODO bitflags
    pub load_segment: U16<LittleEndian>,
    pub system_ty: u8,
    _reserved1: u8,                      // must be 0
    pub sector_count: U16<LittleEndian>, // 512-byte "virtual" sectors (2048-byte ISO sector = 4 virtual sectors)
    pub load_rba: U32<LittleEndian>,     // 2048-byte sectors (ISO LBAs)
    _reserved2: [u8; 20],                // must be 0
}
const _: () = assert!(size_of::<InitialEntry>() == 32);

impl InitialEntry {
    pub fn emulation(&self) -> EmulationType {
        emulation_from_u8(self.boot_media_ty)
    }
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct SectionHeaderEntry {
    pub header_indicator: u8,
    pub platform_id: u8,
    pub entries: U16<LittleEndian>,
    pub id: [u8; 28],
}
const _: () = assert!(size_of::<SectionHeaderEntry>() == 32);

impl SectionHeaderEntry {
    pub fn platform(&self) -> BootPlatform {
        platform_from_u8(self.platform_id)
    }
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct SectionEntry {
    pub bootable: u8,      // 88 = Bootable, 00 = Not Bootable
    pub boot_media_ty: u8, // TODO bitflags
    pub load_segment: U16<LittleEndian>,
    pub system_ty: u8,
    _reserved: u8, // must be 0
    pub sector_count: U16<LittleEndian>,
    pub load_rba: U32<LittleEndian>,
    pub selection_criteria: u8, // 0 = No selection criteria, 1 = Language and Version Information (IBM), 2-FF = Reserved
    pub vendor_selection_criteria: [u8; 19],
}
const _: () = assert!(size_of::<SectionEntry>() == 32);

impl SectionEntry {
    pub fn emulation(&self) -> EmulationType {
        emulation_from_u8(self.boot_media_ty)
    }
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct SectionEntryExtension {
    pub extension_indicator: u8, // Must be 44
    pub bits: u8,
    pub vendor_selection_criteria: [u8; 30],
}
const _: () = assert!(size_of::<SectionEntryExtension>() == 32);
