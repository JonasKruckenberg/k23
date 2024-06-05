#[derive(Debug, onlyerror::Error)]
pub enum Error {
    #[error("Failed")]
    Failed,
    #[error("Not supported")]
    NotSupported,
    #[error("Invalid parameter(s)")]
    InvalidParam,
    #[error("Denied or not allowed")]
    Denied,
    #[error("Invalid address(s)")]
    InvalidAddress,
    #[error("Already available")]
    AlreadyAvailable,
    #[error("Already started")]
    AlreadyStarted,
    #[error("Already stopped")]
    AlreadyStopped,
    #[error("No shared memory available")]
    NoShmem,
    #[error("Other error: {0}")]
    Other(isize),
    #[error("Failed to convert int {0}")]
    IntConversion(#[from] core::num::TryFromIntError),
}
