use core::fmt::{Display, Formatter};

#[derive(Debug)]
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

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Gimli(err) => write!(f, "Gimli error: {err}"),
            Error::ForeignException => write!(f, "Rust cannot catch foreign exceptions"),
            Error::EndOfStack => write!(f, "End of stack"),
            Error::DifferentPersonality => write!(
                f,
                "The personality function is not a Rust personality function"
            ),
            Error::MissingSection(err) => write!(f, "Missing section: {err}"),
        }
    }
}

impl core::error::Error for Error {}
