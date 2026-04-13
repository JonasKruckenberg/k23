use core::mem::size_of;

use zerocopy::{FromBytes, Immutable, KnownLayout};

use super::ParseError;
use crate::raw::{
    BootRecord, EnhancedVolumeDescriptor, PrimaryVolumeDescriptor, SECTOR_SIZE,
    SupplementaryVolumeDescriptor, VolumeDescriptorHeader, VolumeDescriptorSet,
    VolumePartitionDescriptor,
};
use crate::validate::Validate;

pub(crate) struct Parser<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) pos: usize,
    /// When `true`, every `read_validated` call checks semantic invariants and
    /// returns `ParseError::Invalid` on violation.  When `false`, validation is
    /// skipped (lenient / "best-effort" mode for real-world images that bend the
    /// spec).
    pub(crate) strict: bool,
}

impl<'a> Parser<'a> {
    /// Creates a strict parser (validation on by default).
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            strict: true,
        }
    }

    /// Creates a lenient parser (validation disabled).
    pub(crate) fn lenient(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            strict: false,
        }
    }

    pub(crate) fn from_lba_and_len(
        data: &'a [u8],
        lba: u32,
        len: u32,
        strict: bool,
    ) -> Result<Self, ParseError> {
        let data = lba_to_slice(data, lba, len)?;
        Ok(Self {
            data,
            pos: 0,
            strict,
        })
    }

    #[inline]
    pub(crate) fn bytes(&mut self, num: usize) -> Result<&'a [u8], ParseError> {
        let end = self.pos.checked_add(num).unwrap();
        let bytes = self.data.get(self.pos..end).unwrap_or_else(|| {
            panic!(
                "index out of bounds {}..{end} for 0..{}",
                self.pos,
                self.data.len()
            )
        });
        self.pos += num;
        Ok(bytes)
    }

    pub(crate) fn byte_array<const N: usize>(&mut self) -> Result<&'a [u8; N], ParseError> {
        let bytes = self.bytes(N)?;
        // Safety: `bytes` ensures the returned slice is exactly of len `N`
        Ok(unsafe { bytes.try_into().unwrap_unchecked() })
    }

    pub(crate) fn into_rest(self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    pub(crate) fn read<T>(&mut self) -> Result<&'a T, ParseError>
    where
        T: FromBytes + Immutable + KnownLayout,
    {
        let bytes = self.bytes(size_of::<T>())?;
        Ok(T::ref_from_bytes(bytes).unwrap())
    }

    /// Like [`read`], but also runs [`Validate::validate`] when the parser is
    /// in strict mode.  Returns `ParseError::Invalid` on validation failure.
    pub(crate) fn read_validated<T>(&mut self) -> Result<&'a T, ParseError>
    where
        T: FromBytes + Immutable + KnownLayout + Validate,
    {
        let val = self.read::<T>()?;
        if self.strict {
            val.validate().map_err(ParseError::Invalid)?;
        }
        Ok(val)
    }

    pub(crate) fn peek<T>(&self) -> Result<&'a T, ParseError>
    where
        T: FromBytes + Immutable + KnownLayout,
    {
        let end = self.pos.checked_add(size_of::<T>()).unwrap();
        let bytes = self.data.get(self.pos..end).unwrap();
        Ok(T::ref_from_bytes(bytes).unwrap())
    }

    pub(crate) fn volume_descriptor_set(&mut self) -> Result<VolumeDescriptorSet<'a>, ParseError> {
        let mut primary = None;
        let mut boot = Vec::new();
        let mut supplementary = Vec::new();
        let mut enhanced = Vec::new();
        let mut volume_partition = Vec::new();

        loop {
            assert!(
                self.pos % SECTOR_SIZE == 0,
                "descriptor must be aligned on a sector boundary {}",
                self.pos % SECTOR_SIZE
            );
            let header = self.read_validated::<VolumeDescriptorHeader>()?;

            match header.volume_descriptor_ty {
                0 => {
                    boot.push(self.read_validated::<BootRecord>()?);
                }
                1 => {
                    primary = Some(self.read_validated::<PrimaryVolumeDescriptor>()?);
                }
                2 => {
                    let vd = self.peek::<EnhancedVolumeDescriptor>()?;

                    if vd.file_structure_version == 1 {
                        supplementary.push(self.read_validated::<SupplementaryVolumeDescriptor>()?);
                    } else {
                        enhanced.push(self.read_validated::<EnhancedVolumeDescriptor>()?);
                    }
                }
                3 => {
                    volume_partition.push(self.read_validated::<VolumePartitionDescriptor>()?);
                }
                255 => {
                    self.byte_array::<{ SECTOR_SIZE - size_of::<VolumeDescriptorHeader>() }>()?;
                    break;
                }
                found => panic!("unknown volume descriptor type {found}"),
            }
        }

        Ok(VolumeDescriptorSet {
            primary: primary.unwrap(),
            boot,
            supplementary,
            enhanced,
            volume_partition,
        })
    }
}

pub(crate) fn lba_to_slice(data: &[u8], lba: u32, len: u32) -> Result<&[u8], ParseError> {
    let start = (lba as usize).checked_mul(SECTOR_SIZE).unwrap();
    let end = start.checked_add(len as usize).unwrap();

    Ok(data
        .get(start..end)
        .unwrap_or_else(|| panic!("{start}..{end} out of bounds for data (len {})", data.len())))
}
