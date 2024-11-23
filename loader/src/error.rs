#[derive(Debug, onlyerror::Error)]
pub enum Error {
    /// Failed to set up page tables
    Kmm(#[from] kmm::Error),
    /// Failed to convert number
    TryFromInt(#[from] core::num::TryFromIntError),
    /// Failed to parse device tree blob
    Dtb(#[from] dtb_parser::Error),
    /// Failed to parse kernel ELF
    Elf(&'static str),
}