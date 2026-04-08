use core::mem::size_of;
use core::str::FromStr;

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

use super::str::DStr;
use crate::validate::Validate;

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct DecDateTime {
    pub year: DStr<4>,
    pub month: DStr<2>,
    pub day: DStr<2>,
    pub hour: DStr<2>,
    pub minute: DStr<2>,
    pub second: DStr<2>,
    pub hundredth: DStr<2>,
    pub timezone_offset: i8,
}
const _: () = assert!(size_of::<DecDateTime>() == 17);

impl DecDateTime {
    fn validate_field<const N: usize>(field: &DStr<N>, min: u16, max: u16) -> anyhow::Result<()> {
        let s = field.as_str().context("field must be valid ASCII")?;
        let num: u16 = u16::from_str(s.trim()).context("field must be decimal digits")?;
        anyhow::ensure!(
            num >= min && num <= max,
            "field value {num} out of range {min}..={max}"
        );
        Ok(())
    }
}

use anyhow::Context as _;

impl Validate for DecDateTime {
    fn validate(&self) -> anyhow::Result<()> {
        if self.year.0 == [0, 0, 0, 0]
            && self.month.0 == [0, 0]
            && self.day.0 == [0, 0]
            && self.hour.0 == [0, 0]
            && self.minute.0 == [0, 0]
            && self.second.0 == [0, 0]
            && self.hundredth.0 == [0, 0]
            && self.timezone_offset == 0
        {
            return Ok(()); // signifies an absent date
        }

        Self::validate_field(&self.year, 1, 9999).context("DecDateTime.year")?;
        Self::validate_field(&self.month, 1, 12).context("DecDateTime.month")?;
        Self::validate_field(&self.day, 1, 31).context("DecDateTime.day")?;
        Self::validate_field(&self.hour, 0, 23).context("DecDateTime.hour")?;
        Self::validate_field(&self.minute, 0, 59).context("DecDateTime.minute")?;
        Self::validate_field(&self.second, 0, 59).context("DecDateTime.second")?;
        Self::validate_field(&self.hundredth, 0, 99).context("DecDateTime.hundredth")?;
        anyhow::ensure!(
            self.timezone_offset >= -48 && self.timezone_offset <= 52,
            "DecDateTime.timezone_offset: value {} out of range -48..=52",
            self.timezone_offset,
        );
        Ok(())
    }
}

#[derive(Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct DirDateTime {
    pub year: u8,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub timezone_offset: i8,
}
const _: () = assert!(size_of::<DirDateTime>() == 7);

impl DirDateTime {
    fn validate_field(field: u8, min: u8, max: u8) -> anyhow::Result<()> {
        anyhow::ensure!(
            field >= min && field <= max,
            "value {field} out of range {min}..={max}"
        );
        Ok(())
    }
}

impl Validate for DirDateTime {
    fn validate(&self) -> anyhow::Result<()> {
        if self.year == 0
            && self.month == 0
            && self.day == 0
            && self.hour == 0
            && self.minute == 0
            && self.second == 0
            && self.timezone_offset == 0
        {
            return Ok(()); // signifies an absent date
        }

        Self::validate_field(self.month, 1, 12).context("DirDateTime.month")?;
        Self::validate_field(self.day, 1, 31).context("DirDateTime.day")?;
        Self::validate_field(self.hour, 0, 23).context("DirDateTime.hour")?;
        Self::validate_field(self.minute, 0, 59).context("DirDateTime.minute")?;
        Self::validate_field(self.second, 0, 59).context("DirDateTime.second")?;
        anyhow::ensure!(
            self.timezone_offset >= -48 && self.timezone_offset <= 52,
            "DirDateTime.timezone_offset: value {} out of range [-48, 52]",
            self.timezone_offset,
        );
        Ok(())
    }
}
