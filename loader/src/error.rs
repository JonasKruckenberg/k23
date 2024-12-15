#[derive(Debug, onlyerror::Error)]
pub enum Error {
    /// Failed to set up mappings
    Pmm(#[from] pmm::Error),
    /// Failed to convert number
    TryFromInt(#[from] core::num::TryFromIntError),
    /// Failed to parse device tree blob
    Dtb(#[from] dtb_parser::Error),
    /// Failed to parse kernel elf
    Elf(&'static str),
}
