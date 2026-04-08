use core::mem::size_of;

use zerocopy::{FromBytes, Immutable, KnownLayout};

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
    /// returns an error on violation.  When `false`, validation is
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
    ) -> anyhow::Result<Self> {
        let data = lba_to_slice(data, lba, len)?;
        Ok(Self {
            data,
            pos: 0,
            strict,
        })
    }

    #[inline]
    pub(crate) fn bytes(&mut self, num: usize) -> anyhow::Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(num)
            .ok_or_else(|| anyhow::anyhow!("offset overflow at pos {}", self.pos))?;
        let bytes = self.data.get(self.pos..end).ok_or_else(|| {
            anyhow::anyhow!(
                "unexpected EOF: need {}..{end} but data is {} bytes",
                self.pos,
                self.data.len()
            )
        })?;
        self.pos += num;
        Ok(bytes)
    }

    pub(crate) fn byte_array<const N: usize>(&mut self) -> anyhow::Result<&'a [u8; N]> {
        let bytes = self.bytes(N)?;
        // Safety: `bytes` ensures the returned slice is exactly of len `N`
        Ok(unsafe { bytes.try_into().unwrap_unchecked() })
    }

    pub(crate) fn read<T>(&mut self) -> anyhow::Result<&'a T>
    where
        T: FromBytes + Immutable + KnownLayout,
    {
        let bytes = self.bytes(size_of::<T>())?;
        Ok(T::ref_from_bytes(bytes).expect("zerocopy: slice length mismatch (bug in bytes())"))
    }

    /// Like [`read`], but also runs [`Validate::validate`] when the parser is
    /// in strict mode.  Returns an error on validation failure.
    pub(crate) fn read_validated<T>(&mut self) -> anyhow::Result<&'a T>
    where
        T: FromBytes + Immutable + KnownLayout + Validate,
    {
        let val = self.read::<T>()?;
        if self.strict {
            val.validate()?;
        }
        Ok(val)
    }

    pub(crate) fn peek<T>(&self) -> anyhow::Result<&'a T>
    where
        T: FromBytes + Immutable + KnownLayout,
    {
        let end = self
            .pos
            .checked_add(size_of::<T>())
            .ok_or_else(|| anyhow::anyhow!("offset overflow at pos {}", self.pos))?;
        let bytes = self.data.get(self.pos..end).ok_or_else(|| {
            anyhow::anyhow!(
                "unexpected EOF: peek at {}..{end} but data is {} bytes",
                self.pos,
                self.data.len()
            )
        })?;
        Ok(T::ref_from_bytes(bytes).expect("zerocopy: slice length mismatch (bug in peek())"))
    }

    pub(crate) fn volume_descriptor_set(&mut self) -> anyhow::Result<VolumeDescriptorSet<'a>> {
        let mut primary = None;
        let mut boot = Vec::new();
        let mut supplementary = Vec::new();
        let mut enhanced = Vec::new();
        let mut volume_partition = Vec::new();

        loop {
            anyhow::ensure!(
                self.pos % SECTOR_SIZE == 0,
                "volume descriptor must be at a sector boundary (pos={}, remainder={})",
                self.pos,
                self.pos % SECTOR_SIZE,
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
                found => anyhow::bail!("unknown volume descriptor type {found}"),
            }
        }

        Ok(VolumeDescriptorSet {
            primary: primary.ok_or_else(|| anyhow::anyhow!("missing primary volume descriptor"))?,
            boot,
            supplementary,
            enhanced,
            volume_partition,
        })
    }
}

pub(crate) fn lba_to_slice(data: &[u8], lba: u32, len: u32) -> anyhow::Result<&[u8]> {
    let start = (lba as usize)
        .checked_mul(SECTOR_SIZE)
        .ok_or_else(|| anyhow::anyhow!("LBA {lba} overflows usize"))?;
    let end = start
        .checked_add(len as usize)
        .ok_or_else(|| anyhow::anyhow!("LBA {lba} + len {len} overflows usize"))?;
    data.get(start..end).ok_or_else(|| {
        anyhow::anyhow!(
            "LBA {lba} (bytes {start}..{end}) out of bounds for data ({} bytes)",
            data.len()
        )
    })
}
