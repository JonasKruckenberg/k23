#[derive(Debug, onlyerror::Error)]
pub enum Error {
    #[error("failed to parse Device Tree Blob")]
    DTB(#[from] dtb_parser::Error),
    #[error("missing board info property: {0}")]
    MissingBordInfo(&'static str),
}
