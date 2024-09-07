//! Read and write DWARF's "Little Endian Base 128" (LEB128) variable length
//! integer encoding.
//!
//! The implementation is a direct translation of the pseudocode in the DWARF 4
//! standard's appendix C.
//!
//! Read and write signed integers:
//!
//! ```
//! use leb128::{Leb128Read, Leb128Write};
//!
//! let mut buf = [0; 1024];
//!
//! // Write to anything that implements `alloc::vec::Vec`.
//! {
//!     let mut writable = &mut buf[..];
//!     writable.write_sleb128(-12345).expect("Should write number");
//! }
//!
//! // Read from anything that implements `alloc::vec::Vec`.
//! let mut readable = &buf[..];
//! let val = readable.read_sleb128().expect("Should read number");
//! assert_eq!(val, -12345);
//! ```
//!
//! Or read and write unsigned integers:
//!
//! ```
//! use leb128::{Leb128Write, Leb128Read};
//!
//! let mut buf = [0; 1024];
//!
//! {
//!     let mut writable = &mut buf[..];
//!     writable.write_uleb128(98765).expect("Should write number");
//! }
//!
//! let mut readable = &buf[..];
//! let val = readable.read_uleb128().expect("Should read number");
//! assert_eq!(val, 98765);
//! ```
#![no_std]

use core::mem;

#[derive(Debug, onlyerror::Error)]
pub enum Error {
    /// Failed to write to the provided buffer.
    UnexpectedEof,
    /// gg
    Overflow,
    /// h
    NotEnoughSpace,
}
type Result<T> = core::result::Result<T, Error>;

pub trait Leb128Read {
    #[doc(hidden)]
    fn read_byte(&mut self) -> Result<u8>;
    fn read_uleb128(&mut self) -> Result<u64>;
    fn read_sleb128(&mut self) -> Result<i64>;
}

impl<'a> Leb128Read for &'a [u8] {
    fn read_byte(&mut self) -> Result<u8> {
        let (byte, rest) = self.split_first().ok_or(Error::UnexpectedEof)?;
        *self = rest;
        Ok(*byte)
    }

    fn read_uleb128(&mut self) -> Result<u64> {
        let mut result = 0;
        let mut shift = 0;

        loop {
            let mut byte = self.read_byte()?;

            if shift == 63 && byte != 0x00 && byte != 0x01 {
                while byte & CONTINUATION_BIT != 0 {
                    byte = self.read_byte()?;
                }
                return Err(Error::Overflow);
            }

            let low_bits = low_bits_of_byte(byte) as u64;
            result |= low_bits << shift;

            if byte & CONTINUATION_BIT == 0 {
                return Ok(result);
            }

            shift += 7;
        }
    }

    fn read_sleb128(&mut self) -> Result<i64> {
        let mut result = 0;
        let mut shift = 0;
        let size = 64;
        let mut byte;

        loop {
            let mut b = self.read_byte()?;
            byte = b;
            if shift == 63 && byte != 0x00 && byte != 0x7f {
                while b & CONTINUATION_BIT != 0 {
                    b = self.read_byte()?;
                }
                return Err(Error::Overflow);
            }

            let low_bits = low_bits_of_byte(byte) as i64;
            result |= low_bits << shift;
            shift += 7;

            if byte & CONTINUATION_BIT == 0 {
                break;
            }
        }

        if shift < size && (SIGN_BIT & byte) == SIGN_BIT {
            // Sign extend the result.
            result |= !0 << shift;
        }

        Ok(result)
    }
}

pub trait Leb128Write {
    fn write_byte(&mut self, val: u8) -> Result<()>;
    fn write_uleb128(&mut self, val: u64) -> Result<usize>;
    fn write_sleb128(&mut self, val: i64) -> Result<usize>;
}

impl<'a> Leb128Write for &'a mut [u8] {
    #[inline]
    fn write_byte(&mut self, val: u8) -> Result<()> {
        let (a, b) = mem::take(self)
            .split_first_mut()
            .ok_or(Error::NotEnoughSpace)?;
        *a = val;
        *self = b;
        Ok(())
    }

    fn write_uleb128(&mut self, mut val: u64) -> Result<usize> {
        let mut bytes_written = 0;
        loop {
            let mut byte = low_bits_of_u64(val);
            val >>= 7;
            if val != 0 {
                // More bytes to come, so set the continuation bit.
                byte |= CONTINUATION_BIT;
            }

            self.write_byte(byte)?;
            bytes_written += 1;

            if val == 0 {
                return Ok(bytes_written);
            }
        }
    }

    fn write_sleb128(&mut self, mut val: i64) -> Result<usize> {
        let mut bytes_written = 0;
        loop {
            let mut byte = val as u8;
            // Keep the sign bit for testing
            val >>= 6;
            let done = val == 0 || val == -1;
            if done {
                byte &= !CONTINUATION_BIT;
            } else {
                // Remove the sign bit
                val >>= 1;
                // More bytes to come, so set the continuation bit.
                byte |= CONTINUATION_BIT;
            }

            self.write_byte(byte)?;
            bytes_written += 1;

            if done {
                return Ok(bytes_written);
            }
        }
    }
}

const CONTINUATION_BIT: u8 = 1 << 7;
const SIGN_BIT: u8 = 1 << 6;

#[inline]
fn low_bits_of_byte(byte: u8) -> u8 {
    byte & !CONTINUATION_BIT
}
#[inline]
fn low_bits_of_u64(val: u64) -> u8 {
    let byte = val & (u8::MAX as u64);
    low_bits_of_byte(byte as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[ktest::test]
    fn test_low_bits_of_byte() {
        for i in 0..127 {
            assert_eq!(i, low_bits_of_byte(i));
            assert_eq!(i, low_bits_of_byte(i | CONTINUATION_BIT));
        }
    }

    #[ktest::test]
    fn test_low_bits_of_u64() {
        for i in 0u64..127 {
            assert_eq!(i as u8, low_bits_of_u64(1 << 16 | i));
            assert_eq!(
                i as u8,
                low_bits_of_u64(i << 16 | i | (CONTINUATION_BIT as u64))
            );
        }
    }

    // Examples from the DWARF 4 standard, section 7.6, figure 22.
    #[ktest::test]
    fn test_read_unsigned() {
        let buf = [2u8];
        let mut readable = &buf[..];
        assert_eq!(2, readable.read_uleb128().expect("Should read number"));

        let buf = [127u8];
        let mut readable = &buf[..];
        assert_eq!(127, readable.read_uleb128().expect("Should read number"));

        let buf = [CONTINUATION_BIT, 1];
        let mut readable = &buf[..];
        assert_eq!(128, readable.read_uleb128().expect("Should read number"));

        let buf = [1u8 | CONTINUATION_BIT, 1];
        let mut readable = &buf[..];
        assert_eq!(129, readable.read_uleb128().expect("Should read number"));

        let buf = [2u8 | CONTINUATION_BIT, 1];
        let mut readable = &buf[..];
        assert_eq!(130, readable.read_uleb128().expect("Should read number"));

        let buf = [57u8 | CONTINUATION_BIT, 100];
        let mut readable = &buf[..];
        assert_eq!(12857, readable.read_uleb128().expect("Should read number"));
    }

    // Examples from the DWARF 4 standard, section 7.6, figure 23.
    #[ktest::test]
    fn test_read_signed() {
        let buf = [2u8];
        let mut readable = &buf[..];
        assert_eq!(2, readable.read_sleb128().expect("Should read number"));

        let buf = [0x7eu8];
        let mut readable = &buf[..];
        assert_eq!(-2, readable.read_sleb128().expect("Should read number"));

        let buf = [127u8 | CONTINUATION_BIT, 0];
        let mut readable = &buf[..];
        assert_eq!(127, readable.read_sleb128().expect("Should read number"));

        let buf = [1u8 | CONTINUATION_BIT, 0x7f];
        let mut readable = &buf[..];
        assert_eq!(-127, readable.read_sleb128().expect("Should read number"));

        let buf = [CONTINUATION_BIT, 1];
        let mut readable = &buf[..];
        assert_eq!(128, readable.read_sleb128().expect("Should read number"));

        let buf = [CONTINUATION_BIT, 0x7f];
        let mut readable = &buf[..];
        assert_eq!(-128, readable.read_sleb128().expect("Should read number"));

        let buf = [1u8 | CONTINUATION_BIT, 1];
        let mut readable = &buf[..];
        assert_eq!(129, readable.read_sleb128().expect("Should read number"));

        let buf = [0x7fu8 | CONTINUATION_BIT, 0x7e];
        let mut readable = &buf[..];
        assert_eq!(-129, readable.read_sleb128().expect("Should read number"));
    }

    #[ktest::test]
    fn test_read_signed_63_bits() {
        let buf = [
            CONTINUATION_BIT,
            CONTINUATION_BIT,
            CONTINUATION_BIT,
            CONTINUATION_BIT,
            CONTINUATION_BIT,
            CONTINUATION_BIT,
            CONTINUATION_BIT,
            CONTINUATION_BIT,
            0x40,
        ];
        let mut readable = &buf[..];
        assert_eq!(
            -0x4000000000000000,
            readable.read_sleb128().expect("Should read number")
        );
    }

    #[ktest::test]
    fn test_read_unsigned_not_enough_data() {
        let buf = [CONTINUATION_BIT];
        let mut readable = &buf[..];
        let res = readable.read_uleb128();
        assert!(matches!(res, Err(Error::UnexpectedEof)));
    }

    #[ktest::test]
    fn test_read_signed_not_enough_data() {
        let buf = [CONTINUATION_BIT];
        let mut readable = &buf[..];
        let res = readable.read_sleb128();
        assert!(matches!(res, Err(Error::UnexpectedEof)));
    }

    #[ktest::test]
    fn test_write_unsigned_not_enough_space() {
        let mut buf = [0; 1];
        let mut writable = &mut buf[..];
        let res = writable.write_uleb128(128);
        assert!(matches!(res, Err(Error::NotEnoughSpace)));
    }

    #[ktest::test]
    fn test_write_signed_not_enough_space() {
        let mut buf = [0; 1];
        let mut writable = &mut buf[..];
        let res = writable.write_sleb128(128);
        assert!(matches!(res, Err(Error::NotEnoughSpace)));
    }

    #[ktest::test]
    fn dogfood_signed() {
        fn inner(i: i64) {
            let mut buf = [0u8; 1024];

            {
                let mut writable = &mut buf[..];
                writable.write_sleb128(i).expect("Should write number");
            }

            let mut readable = &buf[..];
            let result = readable.read_sleb128().expect("Should read number");
            assert_eq!(i, result);
        }
        for i in -513..513 {
            inner(i);
        }
        inner(i64::MIN);
    }

    #[ktest::test]
    fn dogfood_unsigned() {
        for i in 0..1025 {
            let mut buf = [0u8; 1024];

            {
                let mut writable = &mut buf[..];
                writable.write_uleb128(i).expect("Should write number");
            }

            let mut readable = &buf[..];
            let result = readable.read_uleb128().expect("Should read number");
            assert_eq!(i, result);
        }
    }

    #[ktest::test]
    fn test_read_unsigned_overflow() {
        let buf = [
            2u8 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            1,
        ];
        let mut readable = &buf[..];
        assert!(readable.read_uleb128().is_err());
    }

    #[ktest::test]
    fn test_read_signed_overflow() {
        let buf = [
            2u8 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            2 | CONTINUATION_BIT,
            1,
        ];
        let mut readable = &buf[..];
        assert!(readable.read_sleb128().is_err());
    }

    #[ktest::test]
    fn test_read_multiple() {
        let buf = [2u8 | CONTINUATION_BIT, 1u8, 1u8];

        let mut readable = &buf[..];
        assert_eq!(readable.read_uleb128().expect("Should read number"), 130u64);
        assert_eq!(readable.read_uleb128().expect("Should read number"), 1u64);
    }

    #[ktest::test]
    fn test_read_multiple_with_overflow() {
        let buf = [
            0b1111_1111,
            0b1111_1111,
            0b1111_1111,
            0b1111_1111,
            0b1111_1111,
            0b1111_1111,
            0b1111_1111,
            0b1111_1111,
            0b1111_1111,
            0b1111_1111,
            0b0111_1111, // Overflow!
            0b1110_0100,
            0b1110_0000,
            0b0000_0010, // 45156
        ];
        let mut readable: &[u8] = &buf[..];
        let res = readable.read_uleb128();
        assert!(matches!(res, Err(Error::Overflow)));
        assert_eq!(
            readable
                .read_uleb128()
                .expect("Should succeed with correct value"),
            45156
        );
    }
}
