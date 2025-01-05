use core::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum Error {
    InvalidFieldValue {
        field: &'static str,
        value: usize,
        bitmask: usize,
    },
    IndexOutOfBounds {
        index: usize,
        min: i32,
        max: i32,
    },
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::InvalidFieldValue { .. } => f.write_str("Failed"),
            Error::IndexOutOfBounds { .. } => f.write_str("Failed"),
        }
    }
}

impl core::error::Error for Error {}
