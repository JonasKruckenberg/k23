//! El Torito boot catalog support — extension to ECMA-119 for bootable CDs.
//!
//! Layout mirrors the rest of the crate:
//!  - [`raw`] holds the on-disk types.
//!  - [`parse`] holds the boot catalog iterator and its yielded entries.

mod parse;
pub mod raw;

pub use parse::{BootCatalogIter, CatalogEntry};
pub use raw::{
    InitialEntry, SectionEntry, SectionEntryExtension, SectionHeaderEntry, ValidationEntry,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootPlatform {
    X86Bios, // 0
    PowerPC, // 1
    Mac,     // 2
    Efi,     // 0xEF
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmulationType {
    NoEmulation, // 0
    Floppy12,    // 1
    Floppy144,   // 2
    Floppy288,   // 3
    HardDisk,    // 4
    Unknown(u8),
}

pub(crate) fn platform_from_u8(b: u8) -> BootPlatform {
    match b {
        0 => BootPlatform::X86Bios,
        1 => BootPlatform::PowerPC,
        2 => BootPlatform::Mac,
        0xEF => BootPlatform::Efi,
        x => BootPlatform::Unknown(x),
    }
}

pub(crate) fn emulation_from_u8(b: u8) -> EmulationType {
    // low nibble selects the emulation type; upper bits are flags we ignore here
    match b & 0x0F {
        0 => EmulationType::NoEmulation,
        1 => EmulationType::Floppy12,
        2 => EmulationType::Floppy144,
        3 => EmulationType::Floppy288,
        4 => EmulationType::HardDisk,
        x => EmulationType::Unknown(x),
    }
}
