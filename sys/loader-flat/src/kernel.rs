// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use loader_common::ImageSource;

/// The inlined kernel
static INLINED_KERNEL_BYTES: KernelBytes = KernelBytes(*include_bytes!(env!("KERNEL")));
/// Wrapper type for the inlined bytes to ensure proper alignment
#[repr(C, align(4096))]
struct KernelBytes(pub [u8; include_bytes!(env!("KERNEL")).len()]);

/// The inlined kernel debuginfo.
///
/// Contains the `.symtab` and `.debug_*` sections stripped from the kernel binary.
/// The loader hands a pointer to this blob to the kernel via [`BootInfo::kernel_debuginfo_phys`]
/// so the backtrace subsystem can resolve symbols.
///
/// [`BootInfo::kernel_debuginfo_phys`]: loader_api::BootInfo::kernel_debuginfo_phys
static INLINED_KERNEL_DEBUGINFO_BYTES: KernelDebuginfoBytes =
    KernelDebuginfoBytes(*include_bytes!(env!("KERNEL_DEBUGINFO")));
#[repr(C, align(4096))]
struct KernelDebuginfoBytes(pub [u8; include_bytes!(env!("KERNEL_DEBUGINFO")).len()]);

pub fn locate() -> (SliceSource, SliceSource) {
    (
        SliceSource(INLINED_KERNEL_BYTES.0.as_slice()),
        SliceSource(INLINED_KERNEL_DEBUGINFO_BYTES.0.as_slice()),
    )
}

pub struct SliceSource(&'static [u8]);

impl ImageSource for SliceSource {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_at(&mut self, offset: u64, dst: &mut [u8]) -> loader_common::Result<()> {
        let offset = usize::try_from(offset)?;
        let src = self
            .0
            .get(offset..offset + dst.len())
            .ok_or(loader_common::Error::FieldOutOfRange)?;

        dst.copy_from_slice(src);

        Ok(())
    }
}
