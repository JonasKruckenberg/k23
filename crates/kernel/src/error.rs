#[derive(Debug, onlyerror::Error)]
pub enum Error {
    #[error("failed to parse Device Tree Blob")]
    DTB(#[from] dtb_parser::Error),
    #[error("missing board info property: {0}")]
    MissingBordInfo(&'static str),
    #[error("SBI call failed: {0}")]
    SBI(#[from] sbicall::Error),
    #[error("kernel memory management error: {0}")]
    Kmem(#[from] kmem::Error),
    #[error("out of memory")]
    OutOfMemory,
}
