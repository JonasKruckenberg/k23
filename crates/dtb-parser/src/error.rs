#[derive(Debug, onlyerror::Error)]
pub enum Error {
    #[error("invalid magic number")]
    InvalidMagic,
    #[error("invalid version")]
    InvalidVersion,
    #[error("invalid token: {0}")]
    InvalidToken(u32),
    #[error("invalid tree nesting")]
    InvalidNesting,
    #[error("unexpected end of file")]
    UnexpectedEOF,
    #[error("failed to parse u32")]
    TryFromSlice(#[from] core::array::TryFromSliceError),
    #[error("failed to parse utf8")]
    Utf8(#[from] core::str::Utf8Error),
}
