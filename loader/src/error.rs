#[derive(Debug, onlyerror::Error)]
pub enum Error {
    #[error("Failed to convert number")]
    TryFromInt(#[from] core::num::TryFromIntError),
    #[error("Failed to parse device tree blob")]
    Dtb(#[from] dtb_parser::Error),
    #[error("Failed to parse kernel Elf file")]
    Elf(&'static str),
    #[error("Failed to set up page tables")]
    Pmm(#[from] pmm::Error),
}
