// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;

use loader_common::{ImageSource, ensure};
use uefi::proto::media::file::{Directory, File, FileAttribute, FileInfo, FileMode, RegularFile};
use uefi::{CStr16, Status, cstr16};

use crate::Error;

pub const KERNEL_PATH: &CStr16 = cstr16!("EFI\\k23\\kernel.elf");
const KERNEL_DEBUGINFO_PATH: &CStr16 = cstr16!("EFI\\k23\\kernel.debug");

/// Locate the kernel payload of disk.
/// When this completes successfully we have found the kernel payload
/// files and opened readable handles to it.
pub fn locate() -> crate::Result<(FileSource, Option<FileSource>)> {
    let mut fs = uefi::boot::get_image_file_system(uefi::boot::image_handle())?;
    let mut root = fs.open_volume()?;

    let kernel = FileSource::open(&mut root, KERNEL_PATH)?.ok_or(Error::NoKernel)?;
    let debug_info = FileSource::open(&mut root, KERNEL_DEBUGINFO_PATH)?;

    Ok((kernel, debug_info))
}

pub struct FileSource(RegularFile, Box<FileInfo>);

impl FileSource {
    fn open(root: &mut Directory, path: &CStr16) -> crate::Result<Option<Self>> {
        let res = root
            .open(path, FileMode::Read, FileAttribute::empty())
            .map_err(uefi::Error::split);

        let file = match res {
            Ok(file) => file,
            Err((Status::NOT_FOUND, _)) => return Ok(None),
            Err((status, data)) => return Err(Error::from(uefi::Error::new(status, data))),
        };

        let Some(mut file) = file.into_regular_file() else {
            return Ok(None);
        };

        let info = file.get_boxed_info::<FileInfo>()?;

        Ok(Some(Self(file, info)))
    }
}

impl ImageSource for FileSource {
    fn len(&self) -> u64 {
        self.1.file_size()
    }

    fn read_at(&mut self, offset: u64, dst: &mut [u8]) -> loader_common::Result<()> {
        self.0
            .set_position(offset)
            .map_err(|_| loader_common::Error::FieldOutOfRange)?;

        let bytes_read = self
            .0
            .read(dst)
            .map_err(|_| loader_common::Error::MalformedImage)?;

        ensure!(
            bytes_read == dst.len(),
            loader_common::Error::MalformedImage
        );

        Ok(())
    }
}
