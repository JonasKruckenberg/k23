// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! ECMA-119 primitive types with both-endian encoding.
//!
//! The ISO 9660 standard specifies three ways to encode 16 and 32-bit integers:
//! - Little-endian (LSB first) - Section 7.2.1/7.3.1
//! - Big-endian (MSB first) - Section 7.2.2/7.3.2
//! - Both-endian (LSB followed by MSB) - Section 7.2.3/7.3.3
//!
//! Path tables use either little-endian or big-endian, while volume descriptors
//! and directory records use both-endian format.

use core::fmt;

/// Sector size in bytes (2048 bytes = 2 KiB).
pub const SECTOR_SIZE: usize = 2048;

/// Size of the system area in sectors (16 sectors = 32 KiB).
pub const SYSTEM_AREA_SECTORS: u32 = 16;

/// Size of the system area in bytes.
pub const SYSTEM_AREA_SIZE: usize = SYSTEM_AREA_SECTORS as usize * SECTOR_SIZE;

/// A 16-bit integer stored in both-endian format (LSB-MSB).
///
/// This type stores both little-endian and big-endian representations
/// of the same 16-bit value, as required by ECMA-119 Section 7.2.3.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[repr(C, packed)]
pub struct BothEndian16 {
    /// Little-endian representation
    le: [u8; 2],
    /// Big-endian representation
    be: [u8; 2],
}

impl BothEndian16 {
    /// Creates a new both-endian 16-bit integer.
    #[must_use]
    pub const fn new(value: u16) -> Self {
        Self {
            le: value.to_le_bytes(),
            be: value.to_be_bytes(),
        }
    }

    /// Returns the value as a native integer.
    #[must_use]
    pub const fn get(self) -> u16 {
        u16::from_le_bytes(self.le)
    }

    /// Returns a reference to the little-endian bytes.
    #[must_use]
    pub const fn le_bytes(&self) -> &[u8; 2] {
        &self.le
    }

    /// Returns a reference to the big-endian bytes.
    #[must_use]
    pub const fn be_bytes(&self) -> &[u8; 2] {
        &self.be
    }
}

impl From<u16> for BothEndian16 {
    fn from(value: u16) -> Self {
        Self::new(value)
    }
}

impl From<BothEndian16> for u16 {
    fn from(value: BothEndian16) -> Self {
        value.get()
    }
}

impl fmt::Debug for BothEndian16 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BothEndian16({})", self.get())
    }
}

/// A 32-bit integer stored in both-endian format (LSB-MSB).
///
/// This type stores both little-endian and big-endian representations
/// of the same 32-bit value, as required by ECMA-119 Section 7.3.3.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[repr(C, packed)]
pub struct BothEndian32 {
    /// Little-endian representation
    le: [u8; 4],
    /// Big-endian representation
    be: [u8; 4],
}

impl BothEndian32 {
    /// Creates a new both-endian 32-bit integer.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self {
            le: value.to_le_bytes(),
            be: value.to_be_bytes(),
        }
    }

    /// Returns the value as a native integer.
    #[must_use]
    pub const fn get(self) -> u32 {
        u32::from_le_bytes(self.le)
    }

    /// Returns a reference to the little-endian bytes.
    #[must_use]
    pub const fn le_bytes(&self) -> &[u8; 4] {
        &self.le
    }

    /// Returns a reference to the big-endian bytes.
    #[must_use]
    pub const fn be_bytes(&self) -> &[u8; 4] {
        &self.be
    }
}

impl From<u32> for BothEndian32 {
    fn from(value: u32) -> Self {
        Self::new(value)
    }
}

impl From<BothEndian32> for u32 {
    fn from(value: BothEndian32) -> Self {
        value.get()
    }
}

impl fmt::Debug for BothEndian32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BothEndian32({})", self.get())
    }
}

/// Date and time format used in directory records (7 bytes).
///
/// This is the short date/time format defined in ECMA-119 Section 9.1.5.
/// All values except GMT offset are binary values.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[repr(C, packed)]
pub struct DirectoryDateTime {
    /// Number of years since 1900
    pub years_since_1900: u8,
    /// Month of the year (1-12)
    pub month: u8,
    /// Day of the month (1-31)
    pub day: u8,
    /// Hour of the day (0-23)
    pub hour: u8,
    /// Minute of the hour (0-59)
    pub minute: u8,
    /// Second of the minute (0-59)
    pub second: u8,
    /// Offset from GMT in 15 minute intervals (-48 to +52)
    pub gmt_offset: i8,
}

impl DirectoryDateTime {
    /// Creates a new directory date/time from components.
    #[must_use]
    pub const fn new(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
        gmt_offset: i8,
    ) -> Self {
        Self {
            years_since_1900: (year - 1900) as u8,
            month,
            day,
            hour,
            minute,
            second,
            gmt_offset,
        }
    }

    /// Creates a date/time representing "not specified" (all zeros).
    #[must_use]
    pub const fn unspecified() -> Self {
        Self {
            years_since_1900: 0,
            month: 0,
            day: 0,
            hour: 0,
            minute: 0,
            second: 0,
            gmt_offset: 0,
        }
    }

    /// Returns the year (1900-2155).
    #[must_use]
    pub const fn year(&self) -> u16 {
        self.years_since_1900 as u16 + 1900
    }
}

impl fmt::Debug for DirectoryDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} GMT{:+}",
            self.year(),
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second,
            i16::from(self.gmt_offset) * 15 / 60
        )
    }
}

/// Date and time format used in volume descriptors (17 bytes).
///
/// This is the long date/time format defined in ECMA-119 Section 8.4.26.1.
/// All fields except GMT offset are ASCII digit strings.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(C, packed)]
pub struct VolumeDateTime {
    /// Year from 1 to 9999 (4 ASCII digits)
    pub year: [u8; 4],
    /// Month from 1 to 12 (2 ASCII digits)
    pub month: [u8; 2],
    /// Day from 1 to 31 (2 ASCII digits)
    pub day: [u8; 2],
    /// Hour from 0 to 23 (2 ASCII digits)
    pub hour: [u8; 2],
    /// Minute from 0 to 59 (2 ASCII digits)
    pub minute: [u8; 2],
    /// Second from 0 to 59 (2 ASCII digits)
    pub second: [u8; 2],
    /// Hundredths of a second from 0 to 99 (2 ASCII digits)
    pub centiseconds: [u8; 2],
    /// Offset from GMT in 15 minute intervals (-48 to +52)
    pub gmt_offset: i8,
}

impl VolumeDateTime {
    /// Creates a new volume date/time from components.
    #[must_use]
    pub fn new(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
        centiseconds: u8,
        gmt_offset: i8,
    ) -> Self {
        let mut result = Self::unspecified();
        result.set_year(year);
        result.set_month(month);
        result.set_day(day);
        result.set_hour(hour);
        result.set_minute(minute);
        result.set_second(second);
        result.set_centiseconds(centiseconds);
        result.gmt_offset = gmt_offset;
        result
    }

    /// Creates a date/time representing "not specified".
    ///
    /// Per ECMA-119 Section 8.4.26.1, when not specified all string fields
    /// are ASCII '0' and the GMT offset is binary zero.
    #[must_use]
    pub const fn unspecified() -> Self {
        Self {
            year: [b'0'; 4],
            month: [b'0'; 2],
            day: [b'0'; 2],
            hour: [b'0'; 2],
            minute: [b'0'; 2],
            second: [b'0'; 2],
            centiseconds: [b'0'; 2],
            gmt_offset: 0,
        }
    }

    /// Sets the year field.
    pub fn set_year(&mut self, year: u16) {
        write_decimal_4(&mut self.year, year);
    }

    /// Sets the month field.
    pub fn set_month(&mut self, month: u8) {
        write_decimal_2(&mut self.month, month);
    }

    /// Sets the day field.
    pub fn set_day(&mut self, day: u8) {
        write_decimal_2(&mut self.day, day);
    }

    /// Sets the hour field.
    pub fn set_hour(&mut self, hour: u8) {
        write_decimal_2(&mut self.hour, hour);
    }

    /// Sets the minute field.
    pub fn set_minute(&mut self, minute: u8) {
        write_decimal_2(&mut self.minute, minute);
    }

    /// Sets the second field.
    pub fn set_second(&mut self, second: u8) {
        write_decimal_2(&mut self.second, second);
    }

    /// Sets the centiseconds field.
    pub fn set_centiseconds(&mut self, centiseconds: u8) {
        write_decimal_2(&mut self.centiseconds, centiseconds);
    }
}

impl Default for VolumeDateTime {
    fn default() -> Self {
        Self::unspecified()
    }
}

impl fmt::Debug for VolumeDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}{}{}{}-{}{}-{}{} {}{}:{}{}:{}{}.{}{} GMT{:+}",
            self.year[0] as char,
            self.year[1] as char,
            self.year[2] as char,
            self.year[3] as char,
            self.month[0] as char,
            self.month[1] as char,
            self.day[0] as char,
            self.day[1] as char,
            self.hour[0] as char,
            self.hour[1] as char,
            self.minute[0] as char,
            self.minute[1] as char,
            self.second[0] as char,
            self.second[1] as char,
            self.centiseconds[0] as char,
            self.centiseconds[1] as char,
            i16::from(self.gmt_offset) * 15 / 60
        )
    }
}

/// Writes a 2-digit decimal number as ASCII.
fn write_decimal_2(buf: &mut [u8; 2], value: u8) {
    buf[0] = b'0' + (value / 10);
    buf[1] = b'0' + (value % 10);
}

/// Writes a 4-digit decimal number as ASCII.
fn write_decimal_4(buf: &mut [u8; 4], value: u16) {
    buf[0] = b'0' + (value / 1000) as u8;
    buf[1] = b'0' + ((value / 100) % 10) as u8;
    buf[2] = b'0' + ((value / 10) % 10) as u8;
    buf[3] = b'0' + (value % 10) as u8;
}

/// A fixed-size string buffer padded with spaces.
///
/// ISO 9660 uses space-padded strings for identifiers in volume descriptors.
/// This type provides a convenient way to create such strings.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct StrA<const N: usize> {
    bytes: [u8; N],
}

impl<const N: usize> StrA<N> {
    /// Creates a new string buffer filled with spaces.
    #[must_use]
    pub const fn empty() -> Self {
        Self { bytes: [b' '; N] }
    }

    /// Creates a new string buffer from a string slice.
    ///
    /// The string is truncated if longer than N bytes, and padded with spaces
    /// if shorter.
    #[must_use]
    pub fn from_str(s: &str) -> Self {
        let mut bytes = [b' '; N];
        let copy_len = s.len().min(N);
        bytes[..copy_len].copy_from_slice(&s.as_bytes()[..copy_len]);
        Self { bytes }
    }

    /// Returns the underlying bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; N] {
        &self.bytes
    }

    /// Returns the string content without trailing spaces.
    #[must_use]
    pub fn as_str(&self) -> &str {
        let trimmed = self
            .bytes
            .iter()
            .rposition(|&b| b != b' ')
            .map(|i| i + 1)
            .unwrap_or(0);
        // SAFETY: We only allow ASCII characters which are valid UTF-8
        unsafe { core::str::from_utf8_unchecked(&self.bytes[..trimmed]) }
    }
}

impl<const N: usize> Default for StrA<N> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<const N: usize> fmt::Debug for StrA<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\"", self.as_str())
    }
}

impl<const N: usize> AsRef<[u8]> for StrA<N> {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_both_endian_16() {
        let value = BothEndian16::new(0x1234);
        assert_eq!(value.get(), 0x1234);
        assert_eq!(value.le_bytes(), &[0x34, 0x12]);
        assert_eq!(value.be_bytes(), &[0x12, 0x34]);
    }

    #[test]
    fn test_both_endian_32() {
        let value = BothEndian32::new(0x12345678);
        assert_eq!(value.get(), 0x12345678);
        assert_eq!(value.le_bytes(), &[0x78, 0x56, 0x34, 0x12]);
        assert_eq!(value.be_bytes(), &[0x12, 0x34, 0x56, 0x78]);
    }

    #[test]
    fn test_directory_datetime() {
        let dt = DirectoryDateTime::new(2025, 6, 15, 14, 30, 45, 0);
        assert_eq!(dt.year(), 2025);
        assert_eq!(dt.month, 6);
        assert_eq!(dt.day, 15);
    }

    #[test]
    fn test_volume_datetime() {
        let dt = VolumeDateTime::new(2025, 6, 15, 14, 30, 45, 0, 0);
        assert_eq!(&dt.year, b"2025");
        assert_eq!(&dt.month, b"06");
        assert_eq!(&dt.day, b"15");
    }

    #[test]
    fn test_str_a() {
        let s: StrA<32> = StrA::from_str("TEST");
        assert_eq!(s.as_str(), "TEST");
        assert_eq!(&s.as_bytes()[..4], b"TEST");
        assert_eq!(s.as_bytes()[4], b' ');
    }

    #[test]
    fn test_sizes() {
        assert_eq!(size_of::<BothEndian16>(), 4);
        assert_eq!(size_of::<BothEndian32>(), 8);
        assert_eq!(size_of::<DirectoryDateTime>(), 7);
        assert_eq!(size_of::<VolumeDateTime>(), 17);
    }
}
