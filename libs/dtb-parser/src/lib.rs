//! A parser for the device tree blob format.
//!
//! The device tree blob format is a binary format used by firmware to describe non-discoverable
//! hardware to the operating system. This includes things like the number of CPUs and their frequency,
//! MMIO regions, interrupt controllers, and other platform-specific information.
//!
//! The format is described in detail in the [Device Tree Specification](https://github.com/devicetree-org/devicetree-specification);
#![no_std]
#![feature(error_in_core)]

pub mod debug;
mod error;
mod parser;

use core::ffi::CStr;
use core::{mem, slice, str};

use crate::parser::Parser;
pub use error::Error;

type Result<T> = core::result::Result<T, Error>;

const FDT_BEGIN_NODE: u32 = 0x00000001;
const FDT_END_NODE: u32 = 0x00000002;
const FDT_PROP: u32 = 0x00000003;
const FDT_NOP: u32 = 0x00000004;
const FDT_END: u32 = 0x00000009;
const DTB_MAGIC: u32 = 0xD00DFEED;
const DTB_VERSION: u32 = 17;

#[allow(unused_variables)]
pub trait Visitor<'dt> {
    type Error: core::error::Error;

    fn visit_subnode(
        &mut self,
        name: &'dt str,
        node: Node<'dt>,
    ) -> core::result::Result<(), Self::Error> {
        Ok(())
    }

    fn visit_reg(&mut self, reg: &'dt [u8]) -> core::result::Result<(), Self::Error> {
        Ok(())
    }

    fn visit_address_cells(&mut self, cells: u32) -> core::result::Result<(), Self::Error> {
        Ok(())
    }

    fn visit_size_cells(&mut self, cells: u32) -> core::result::Result<(), Self::Error> {
        Ok(())
    }

    fn visit_compatible(&mut self, strings: Strings<'dt>) -> core::result::Result<(), Self::Error> {
        Ok(())
    }

    fn visit_property(
        &mut self,
        name: &'dt str,
        value: &'dt [u8],
    ) -> core::result::Result<(), Self::Error> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct DevTree<'dt> {
    version: u32,
    last_comp_version: u32,
    boot_cpuid_phys: u32,
    total_slice: &'dt [u8],
    memory_slice: &'dt [u8],
    parser: Parser<'dt>,
}

#[derive(Debug)]
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

impl<'dt> DevTree<'dt> {
    /// Parse a device tree blob starting at the given base pointer.
    ///
    /// # Safety
    ///
    /// The caller has to ensure the given pointer is valid and actually points to the device tree blob
    /// as only minimal sanity checking is performed.
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

        let total_slice = {
            let length = u32::from_be_bytes(header.totalsize);
            slice::from_raw_parts(base, length as usize)
        };

        Ok(Self {
            version: u32::from_be_bytes(header.version),
            last_comp_version: u32::from_be_bytes(header.last_comp_version),
            boot_cpuid_phys: u32::from_be_bytes(header.boot_cpuid_phys),
            total_slice,
            memory_slice,
            parser: Parser {
                struct_slice,
                strings_slice,
                level: 0,
                offset: 0,
            },
        })
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn last_comp_version(&self) -> u32 {
        self.last_comp_version
    }

    pub fn boot_cpuid_phys(&self) -> u32 {
        self.boot_cpuid_phys
    }

    pub fn as_slice(&self) -> &'dt [u8] {
        self.total_slice
    }

    pub fn visit<E: core::error::Error + From<Error>>(
        mut self,
        visitor: &mut dyn Visitor<'dt, Error = E>,
    ) -> core::result::Result<(), E> {
        self.parser.visit(visitor)
    }

    pub fn reserved_entries(&self) -> ReserveEntries<'dt> {
        ReserveEntries {
            buf: self.memory_slice,
            offset: 0,
            done: false,
        }
    }
}

#[derive(Clone)]
pub struct Node<'dt> {
    parser: Parser<'dt>,
}

impl<'dt> Node<'dt> {
    fn new(struct_slice: &'dt [u8], strings_slice: &'dt [u8], offset: usize, level: usize) -> Self {
        Self {
            parser: Parser {
                struct_slice,
                strings_slice,
                offset,
                level,
            },
        }
    }

    pub fn visit<E: core::error::Error + From<Error>>(
        mut self,
        visitor: &mut dyn Visitor<'dt, Error = E>,
    ) -> core::result::Result<(), E> {
        self.parser.visit(visitor)
    }
}

pub fn read_str(slice: &[u8], offset: u32) -> Result<&str> {
    let slice = &slice.get(offset as usize..).ok_or(Error::UnexpectedEOF)?;
    let str = CStr::from_bytes_until_nul(slice)?;
    Ok(str.to_str()?)
}

#[derive(Debug)]
pub struct ReserveEntry {
    pub address: u64,
    pub size: u64,
}

pub struct ReserveEntries<'dt> {
    buf: &'dt [u8],
    offset: usize,
    done: bool,
}

impl<'dt> ReserveEntries<'dt> {
    fn read_u64(&mut self) -> Result<u64> {
        let bytes = self
            .buf
            .get(self.offset..self.offset + mem::size_of::<u64>())
            .ok_or(Error::UnexpectedEOF)?;
        self.offset += mem::size_of::<u64>();

        Ok(u64::from_be_bytes(bytes.try_into()?))
    }

    pub fn next_entry(&mut self) -> Result<Option<ReserveEntry>> {
        if self.done || self.offset == self.buf.len() {
            Ok(None)
        } else {
            let entry = {
                let address = self.read_u64()?;
                let size = self.read_u64()?;

                Ok(ReserveEntry { address, size })
            };

            // entries where both address and size is zero mark the end
            let is_empty = entry
                .as_ref()
                .map(|e| e.address == 0 || e.size == 0)
                .unwrap_or_default();

            self.done = entry.is_err() || is_empty;

            if !is_empty {
                entry.map(Some)
            } else {
                Ok(None)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Strings<'dt> {
    bytes: &'dt [u8],
    offset: usize,
    err: bool,
}

impl<'dt> Strings<'dt> {
    pub fn new(bytes: &'dt [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            err: false,
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<&'dt str>> {
        if self.offset == self.bytes.len() || self.err {
            return Ok(None);
        }

        let str = read_str(self.bytes, self.offset as u32)?;
        self.offset += str.len() + 1;

        Ok(Some(str))
    }
}
