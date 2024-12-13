#[derive(Debug, onlyerror::Error)]
pub enum Error {
    /// Failed to set up mappings
    Pmm(#[from] pmm::Error),
    /// Failed to parse device tree blob
    Dtb(#[from] dtb_parser::Error),
}
