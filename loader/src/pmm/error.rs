#[derive(Debug, onlyerror::Error)]
pub enum Error {
    #[error("Out of memory")]
    OutOfMemory,
}