use core::str::{FromStr, Utf8Error};

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use crate::validate::{Validate, is_a_char, is_d_char, is_file_id_char};

// d-characters: A–Z, 0–9, `_` (ECMA-119 §7.4.1).
// Fields are padded with SPACE (`0x20`) to fill the fixed width.
#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, Hash, FromBytes, IntoBytes, Immutable, KnownLayout,
)]
#[repr(transparent)]
pub struct DStr<const N: usize>(pub(crate) [u8; N]);

impl<const N: usize> Validate for DStr<N> {
    fn validate(&self) -> anyhow::Result<()> {
        let bytes = self.0.trim_ascii_end();
        for (i, &b) in bytes.iter().enumerate() {
            anyhow::ensure!(
                is_d_char(b) || b == b' ',
                "DStr: invalid character {b:#04x} ({}) at position {i}",
                char::from(b),
            );
        }
        Ok(())
    }
}

impl<const N: usize> DStr<N> {
    pub fn try_from_bytes(bytes: [u8; N]) -> anyhow::Result<Self> {
        let me = Self(bytes);
        me.validate()?;
        Ok(me)
    }

    pub fn as_str(&self) -> Result<&str, Utf8Error> {
        str::from_utf8(&self.0)
    }
}

impl<const N: usize> FromStr for DStr<N> {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        let bytes = s.as_bytes();
        anyhow::ensure!(
            bytes.len() <= N,
            "DStr<{N}>: input length {} exceeds maximum {N}",
            bytes.len(),
        );
        let mut arr = [b' '; N];
        arr[..bytes.len()].copy_from_slice(bytes);
        Self::try_from_bytes(arr)
    }
}

// a-characters: d-characters plus SPACE and `! " % & ' ( ) * + , - . / : ; < = > ?`
// (ECMA-119 §7.4.2).
// Fields are padded with SPACE (`0x20`) to fill the fixed width.
#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, Hash, FromBytes, IntoBytes, Immutable, KnownLayout,
)]
#[repr(transparent)]
pub struct AStr<const N: usize>(pub(crate) [u8; N]);

impl<const N: usize> Validate for AStr<N> {
    fn validate(&self) -> anyhow::Result<()> {
        let bytes = self.0.trim_ascii_end();
        for (i, &b) in bytes.iter().enumerate() {
            anyhow::ensure!(
                is_a_char(b),
                "AStr: invalid character {b:#04x} ({}) at position {i}",
                char::from(b),
            );
        }
        Ok(())
    }
}

impl<const N: usize> AStr<N> {
    pub fn try_from_bytes(bytes: [u8; N]) -> anyhow::Result<Self> {
        let me = Self(bytes);
        me.validate()?;
        Ok(me)
    }

    pub fn as_str(&self) -> Result<&str, Utf8Error> {
        str::from_utf8(&self.0)
    }
}

impl<const N: usize> FromStr for AStr<N> {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        let bytes = s.as_bytes();
        anyhow::ensure!(
            bytes.len() <= N,
            "AStr<{N}>: input length {} exceeds maximum {N}",
            bytes.len(),
        );
        let mut arr = [b' '; N];
        arr[..bytes.len()].copy_from_slice(bytes);
        Self::try_from_bytes(arr)
    }
}

// d-characters: A–Z, 0–9, `_` (ECMA-119 §7.4.1).
// Fields are padded with SPACE (`0x20`) to fill the fixed width.
#[derive(
    Debug, PartialEq, Eq, PartialOrd, Ord, Hash, FromBytes, IntoBytes, Immutable, KnownLayout,
)]
#[repr(transparent)]
pub struct FileId<const N: usize>(pub(crate) [u8; N]);

impl<const N: usize> Validate for FileId<N> {
    fn validate(&self) -> anyhow::Result<()> {
        let bytes = self.0.trim_ascii_end();
        for (i, &b) in bytes.iter().enumerate() {
            anyhow::ensure!(
                is_file_id_char(b),
                "FileId: invalid character {b:#04x} ({}) at position {N}",
                char::from(b),
            );
        }
        Ok(())
    }
}

impl<const N: usize> FileId<N> {
    pub fn try_from_bytes(bytes: [u8; N]) -> anyhow::Result<Self> {
        let me = Self(bytes);
        me.validate()?;
        Ok(me)
    }

    pub fn as_str(&self) -> Result<&str, Utf8Error> {
        str::from_utf8(&self.0)
    }
}

impl<const N: usize> FromStr for FileId<N> {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        let bytes = s.as_bytes();
        anyhow::ensure!(
            bytes.len() <= N,
            "FileId<{N}>: input length {} exceeds maximum {N}",
            bytes.len(),
        );
        let mut arr = [b' '; N];
        arr[..bytes.len()].copy_from_slice(bytes);
        Self::try_from_bytes(arr)
    }
}
