use std::mem::size_of;

use zerocopy::{FromBytes, Immutable, KnownLayout};

use super::ParseError;
use crate::raw::{
    BootRecord, EnhancedVolumeDescriptor, PrimaryVolumeDescriptor, SECTOR_SIZE,
    SupplementaryVolumeDescriptor, VolumeDescriptorHeader, VolumeDescriptorSet,
    VolumePartitionDescriptor,
};

pub(crate) struct Parser<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) pos: usize,
}

impl<'a> Parser<'a> {
    pub(crate) fn from_lba_and_len(data: &'a [u8], lba: u32, len: u32) -> Result<Self, ParseError> {
        let data = lba_to_slice(data, lba, len)?;

        Ok(Self { data, pos: 0 })
    }

    #[inline]
    pub(crate) fn bytes(&mut self, num: usize) -> Result<&'a [u8], ParseError> {
        let end = self.pos.checked_add(num).unwrap();
        let bytes = self.data.get(self.pos..end).unwrap();
        self.pos += num;
        Ok(bytes)
    }

    pub(crate) fn byte_array<const N: usize>(&mut self) -> Result<&'a [u8; N], ParseError> {
        let bytes = self.bytes(N)?;
        // Safety: `bytes` ensures the returned slice is exactly of len `N`
        Ok(unsafe { bytes.try_into().unwrap_unchecked() })
    }

    pub(crate) fn read<T>(&mut self) -> Result<&'a T, ParseError>
    where
        T: FromBytes + Immutable + KnownLayout,
    {
        let bytes = self.bytes(size_of::<T>())?;
        Ok(T::ref_from_bytes(bytes).unwrap())
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
            let header = self.read::<VolumeDescriptorHeader>()?;

            match header.volume_descriptor_ty {
                0 => {
                    boot.push(self.read::<BootRecord>()?);
                }
                1 => {
                    primary = Some(self.read::<PrimaryVolumeDescriptor>()?);
                }
                2 => {
                    let vd = self.peek::<EnhancedVolumeDescriptor>()?;

                    if vd.file_structure_version == 1 {
                        supplementary.push(self.read::<SupplementaryVolumeDescriptor>()?);
                    } else {
                        enhanced.push(self.read::<EnhancedVolumeDescriptor>()?);
                    }
                }
                3 => {
                    volume_partition.push(self.read::<VolumePartitionDescriptor>()?);
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
