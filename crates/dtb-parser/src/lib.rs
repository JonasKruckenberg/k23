#![no_std]
#![feature(error_in_core)]

mod error;

use core::slice;
pub use error::Error;
pub(crate) type Result<T> = core::result::Result<T, Error>;

const DTB_MAGIC: u32 = 0xD00DFEED;
const DTB_VERSION: u32 = 17;

pub struct Dtb<'a> {
    header: &'a Header,
    memory_slice: &'a [u8],
    struct_slice: &'a [u8],
    strings_slice: &'a [u8],
}

#[repr(C)]
struct Header {
    magic: [u8; 4],
    totalsize: [u8; 4],
    off_dt_struct: [u8; 4],
    off_dt_strings: [u8; 4],
    off_mem_rsvmap: [u8; 4],
    version: [u8; 4],
    last_comp_version: [u8; 4],
    boot_cpuid_phys: [u8; 4],
    size_dt_strings: [u8; 4],
    size_dt_struct: [u8; 4],
}

impl<'a> Dtb<'a> {
    pub unsafe fn from_raw(base: *const u8) -> Result<Self> {
        let header = unsafe { &*(base as *const Header) };

        if u32::from_be_bytes(header.magic) != DTB_MAGIC {
            return Err(Error::InvalidMagic);
        }

        if u32::from_be_bytes(header.version) != DTB_VERSION {
            return Err(Error::InvalidVersion);
        }

        let struct_slice = {
            let addr = base.add(u32::from_be_bytes(header.off_dt_struct) as usize);
            let len = u32::from_be_bytes(header.size_dt_struct) as usize;
            slice::from_raw_parts(addr, len)
        };

        let strings_slice = {
            let addr = base.add(u32::from_be_bytes(header.off_dt_strings) as usize);
            let length = u32::from_be_bytes(header.size_dt_strings) as usize;
            slice::from_raw_parts(addr, length)
        };

        let memory_slice = {
            let addr = base.add(u32::from_be_bytes(header.off_mem_rsvmap) as usize);
            let length =
                u32::from_be_bytes(header.totalsize) - u32::from_be_bytes(header.off_mem_rsvmap);
            slice::from_raw_parts(addr, length as usize)
        };

        Ok(Self {
            header,
            struct_slice,
            strings_slice,
            memory_slice,
        })
    }
}
