// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use super::utils::{deref_pointer, get_unlimited_slice};
use gimli::{BaseAddresses, EhFrame, EhFrameHdr, EndianSlice, NativeEndian, ParsedEhFrameHdr};
use sync::LazyLock;

// Below is a fun hack: We need a reference to the `.eh_frame` and `.eh_frame_hdr` sections and
// must therefore force the linker to retain those even in release builds. By abusing mutable statics
// like below we get a reference to the section start AND force it to not be garbage collected.

#[used(linker)]
#[link_section = ".eh_frame"]
static mut EH_FRAME: [u8; 0] = [];

#[used(linker)]
#[link_section = ".eh_frame_hdr"]
static mut EH_FRAME_HDR: [u8; 0] = [];

#[derive(Debug)]
pub struct EhInfo {
    /// A set of base addresses used for relative addressing.
    pub bases: BaseAddresses,
    /// The parsed `.eh_frame_hdr` section.
    pub hdr: ParsedEhFrameHdr<EndianSlice<'static, NativeEndian>>,
    /// The parsed `.eh_frame` containing the call frame information.
    pub eh_frame: EhFrame<EndianSlice<'static, NativeEndian>>,
}

pub static EH_INFO: LazyLock<EhInfo> = LazyLock::new(|| {
    #[allow(static_mut_refs)]
    let eh_frame_hdr = unsafe { get_unlimited_slice(EH_FRAME_HDR.as_ptr()) };

    let mut bases = BaseAddresses::default().set_eh_frame_hdr(eh_frame_hdr.as_ptr() as u64);
    // .set_text(0xffff_ffff_8000_0000); // TODO support dynamic offsets

    let hdr = EhFrameHdr::new(eh_frame_hdr, NativeEndian)
        .parse(&bases, 8)
        .unwrap();

    let eh_frame = unsafe { deref_pointer(hdr.eh_frame_ptr()) as *const u8 };
    bases = bases.set_eh_frame(eh_frame as u64);

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
