// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use gimli::{
    BaseAddresses, EhFrame, EhFrameHdr, EndianSlice, FrameDescriptionEntry, NativeEndian,
    ParsedEhFrameHdr, UnwindSection,
};
use spin::LazyLock;

use super::utils::{deref_pointer, get_unlimited_slice};

// Below is a fun hack: We need a reference to the `.eh_frame` and `.eh_frame_hdr` sections and
// must therefore force the linker to retain those even in release builds. By abusing mutable statics
// like below we get a reference to the section start AND force it to not be garbage collected.

#[used(linker)]
#[unsafe(link_section = ".eh_frame")]
static mut EH_FRAME: [u8; 0] = [];

#[used(linker)]
#[unsafe(link_section = ".eh_frame_hdr")]
static mut EH_FRAME_HDR: [u8; 0] = [];

#[derive(Debug)]
pub struct EhInfo {
    /// A set of base addresses used for relative addressing.
    pub bases: BaseAddresses,
    /// The parsed `.eh_frame_hdr` section.
    hdr: Option<ParsedEhFrameHdr<EndianSlice<'static, NativeEndian>>>,
    /// The parsed `.eh_frame` containing the call frame information.
    pub eh_frame: EhFrame<EndianSlice<'static, NativeEndian>>,
}

impl EhInfo {
    /// Attempt to lookup up the Frame Descriptor Entry (FDE) for the given address.
    ///
    /// # Errors
    ///
    /// If no FDE for the given address can be found, an error will be returned.
    pub fn fde_for_address(
        &self,
        address: u64,
    ) -> gimli::Result<FrameDescriptionEntry<EndianSlice<'_, NativeEndian>, usize>> {
        if let Some(table) = self.hdr.as_ref().and_then(|hdr| hdr.table()) {
            table.fde_for_address(
                &self.eh_frame,
                &self.bases,
                address,
                EhFrame::cie_from_offset,
            )
        } else {
            self.eh_frame
                .fde_for_address(&self.bases, address, EhFrame::cie_from_offset)
        }
    }
}

pub static EH_INFO: LazyLock<EhInfo> = LazyLock::new(|| {
    // Safety: The start is valid by construction (ensured by the linker) and gimli
    // takes care to never read more than the required bytes from the slice
    #[allow(static_mut_refs, reason = "TODO remove")]
    let eh_frame_hdr = unsafe { get_unlimited_slice(EH_FRAME_HDR.as_ptr()) };

    let mut bases = BaseAddresses::default().set_eh_frame_hdr(eh_frame_hdr.as_ptr() as u64);

    let (hdr, eh_frame) =
        if let Ok(hdr) = EhFrameHdr::new(eh_frame_hdr, NativeEndian).parse(&bases, 8) {
            // Safety: we have to trust the pointer returned by gimli is valid
            let eh_frame = unsafe { deref_pointer(hdr.eh_frame_ptr()) as *const u8 };

            (Some(hdr), eh_frame)
        } else {
            // Safety: The start is valid by construction (ensured by the linker) and gimli
            // takes care to never read more than the required bytes from the slice
            #[allow(static_mut_refs, reason = "TODO remove")]
            let eh_frame = unsafe { EH_FRAME.as_ptr() };

            (None, eh_frame)
        };

    bases = bases.set_eh_frame(eh_frame as u64);

    // Safety: The start is valid by construction (ensured by the linker) and gimli
    // takes care to never read more than the required bytes from the slice
    let eh_frame = EhFrame::new(unsafe { get_unlimited_slice(eh_frame) }, NativeEndian);

    EhInfo {
        bases,
        hdr,
        eh_frame,
    }
});

pub fn obtain_eh_info() -> &'static EhInfo {
    &EH_INFO
}
