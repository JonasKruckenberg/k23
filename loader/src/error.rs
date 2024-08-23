#[derive(Debug, onlyerror::Error)]
pub enum Error {
    /// Failed to set up page tables
    Kmm(#[from] kmm::Error),
    /// Failed to decompress the payload
    Decompression(lz4_flex::block::DecompressError),
    /// Failed to convert number
    TryFromInt(#[from] core::num::TryFromIntError),
    /// Failed to parse device tree blob
    Dtb(#[from] dtb_parser::Error),
    /// Failed to parse payload ELF
    Elf(object::read::Error),
}

impl From<object::read::Error> for Error {
    fn from(err: object::Error) -> Self {
        Self::Elf(err)
    }
}
