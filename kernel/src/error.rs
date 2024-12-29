#[derive(Debug, onlyerror::Error)]
pub enum Error {
    /// Failed to set up mappings
    Mmu(#[from] mmu::Error),
    /// Failed to parse device tree blob
    Dtb(#[from] dtb_parser::Error),
    /// Access to a resource was denied
    AccessDenied,
}
