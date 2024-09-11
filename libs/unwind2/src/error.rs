#[derive(Debug, onlyerror::Error)]
pub enum Error {
    /// Gimli error
    Gimli(gimli::Error),
    /// Rust cannot catch foreign exceptions
    ForeignException,
    /// End of stack
    EndOfStack,
    /// The personality function is not a Rust personality function
    DifferentPersonality,
    /// Missing section
    MissingSection(&'static str),
}

impl From<gimli::Error> for Error {
    fn from(err: gimli::Error) -> Self {
        Error::Gimli(err)
    }
}
