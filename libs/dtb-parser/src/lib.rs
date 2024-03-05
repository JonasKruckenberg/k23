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

use core::{mem, slice, str};

pub use error::Error;

type Result<T> = core::result::Result<T, Error>;

const FDT_BEGIN_NODE: u32 = 0x00000001;
const FDT_END_NODE: u32 = 0x00000002;
const FDT_PROP: u32 = 0x00000003;
const FDT_NOP: u32 = 0x00000004;
const FDT_END: u32 = 0x00000009;
const DTB_MAGIC: u32 = 0xD00DFEED;
const DTB_VERSION: u32 = 17;

fn align_down(value: usize, alignment: usize) -> usize {
    value & !(alignment - 1)
}

fn align_up(value: usize, alignment: usize) -> usize {
    align_down(value + (alignment - 1), alignment)
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

#[derive(Debug)]
pub struct DevTree<'dt> {
    header: &'dt Header,
    memory_slice: &'dt [u8],
    cursor: Cursor<'dt>,
}

impl<'dt> DevTree<'dt> {
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
            memory_slice,
            cursor: Cursor {
                struct_slice,
                strings_slice,
                level: 0,
                offset: 0,
            },
        })
    }

    pub fn visit(mut self, visitor: &mut dyn Visitor<'dt>) -> Result<()> {
        self.cursor.visit(visitor)
    }
}

#[allow(unused_variables)]
pub trait Visitor<'dt> {
    fn visit_subnode(
        &mut self,
        name: &'dt str,
        node: Node<'dt>,
    ) -> core::result::Result<(), Error> {
        Ok(())
    }

    fn visit_reg(&mut self, reg: &'dt [u8]) -> core::result::Result<(), Error> {
        Ok(())
    }

    fn visit_address_cells(&mut self, cells: u32) -> core::result::Result<(), Error> {
        Ok(())
    }

    fn visit_size_cells(&mut self, cells: u32) -> core::result::Result<(), Error> {
        Ok(())
    }

    fn visit_compatible(&mut self, strings: Strings<'dt>) -> core::result::Result<(), Error> {
        Ok(())
    }

    fn visit_property(
        &mut self,
        name: &'dt str,
        value: &'dt [u8],
    ) -> core::result::Result<(), Error> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct Cursor<'dt> {
    struct_slice: &'dt [u8],
    strings_slice: &'dt [u8],
    /// Offset into the struct_slice buffer
    offset: usize,
    level: usize,
}

impl<'dt> Cursor<'dt> {
    fn visit(&mut self, visitor: &mut dyn Visitor<'dt>) -> Result<()> {
        let mut nesting_level = self.level;

        while self.offset < self.struct_slice.len() {
            let token = self.read_u32()?;

            match token {
                FDT_BEGIN_NODE => {
                    nesting_level += 1;

                    let name = read_str(&self.struct_slice, self.offset as u32)?;
                    self.offset += align_up(name.len() + 1, mem::size_of::<u32>());

                    if nesting_level == self.level + 1 {
                        let node = Node::new(
                            self.struct_slice,
                            self.strings_slice,
                            self.offset,
                            nesting_level,
                        );

                        // hack to skip over the root node
                        // if name.is_empty() && self.level == 0 {
                        //     node.visit(visitor)?;
                        // } else {
                        visitor.visit_subnode(name, node)?;
                        // }
                    }
                }
                FDT_END_NODE => {
                    if nesting_level <= self.level {
                        return Ok(());
                    }

                    nesting_level -= 1;
                }
                FDT_PROP => {
                    let len = self.read_u32()? as usize;
                    let nameoff = self.read_u32()?;

                    let aligned_len = align_up(len, mem::size_of::<u32>());

                    let bytes = self.read_bytes(len)?;
                    self.offset += aligned_len - len;

                    if nesting_level != self.level {
                        continue;
                    }

                    let name = read_str(&self.strings_slice, nameoff)?;

                    match name {
                        "reg" => visitor.visit_reg(bytes)?,
                        "#address-cells" => {
                            visitor.visit_address_cells(u32::from_be_bytes(bytes.try_into()?))?;
                        }
                        "#size-cells" => {
                            visitor.visit_size_cells(u32::from_be_bytes(bytes.try_into()?))?;
                        }
                        "compatible" => {
                            visitor.visit_compatible(Strings::new(bytes))?;
                        }
                        _ => visitor.visit_property(name, bytes)?,
                    }
                }
                FDT_NOP => {}
                FDT_END => {
                    return if nesting_level != 0 || self.offset != self.struct_slice.len() {
                        Err(Error::InvalidNesting)?
                    } else {
                        Ok(())
                    };
                }
                _ => return Err(Error::InvalidToken(token)),
            }
        }

        Ok(())
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'dt [u8]> {
        let slice = self
            .struct_slice
            .get(self.offset..self.offset + n)
            .ok_or(Error::UnexpectedEOF)?;
        self.offset += n;

        Ok(slice)
    }

    fn read_u32(&mut self) -> Result<u32> {
        let bytes = self.read_bytes(mem::size_of::<u32>())?;

        Ok(u32::from_be_bytes(bytes.try_into()?))
    }
}

#[derive(Clone)]
pub struct Node<'dt> {
    parser: Cursor<'dt>,
}

impl<'dt> Node<'dt> {
    fn new(struct_slice: &'dt [u8], strings_slice: &'dt [u8], offset: usize, level: usize) -> Self {
        Self {
            parser: Cursor {
                struct_slice,
                strings_slice,
                offset,
                level,
            },
        }
    }

    pub fn visit(mut self, visitor: &mut dyn Visitor<'dt>) -> crate::Result<()> {
        self.parser.visit(visitor)
    }
}

fn read_str<'dt>(slice: &'dt [u8], nameoff: u32) -> Result<&'dt str> {
    let slice = &slice.get(nameoff as usize..).ok_or(Error::UnexpectedEOF)?;
    let name_len = c_strlen_on_slice(slice);
    let name = str::from_utf8(&slice[..name_len])?;
    Ok(name)
}

fn c_strlen_on_slice(slice: &[u8]) -> usize {
    let mut end = slice;
    while !end.is_empty() && *end.first().unwrap_or(&0) != 0 {
        end = &end[1..];
    }

    end.as_ptr() as usize - slice.as_ptr() as usize
}

// #[derive(Debug)]
// pub struct ReserveEntry {
//     pub address: u64,
//     pub size: u64,
// }
//
// pub struct ReserveEntries<'dt> {
//     buf: &'dt [u8],
//     done: bool,
// }
//
// impl<'dt> ReserveEntries<'dt> {
//     fn read_u64(&mut self) -> Result<u64> {
//         let (buf, rest) = self.buf.split_at(mem::size_of::<u64>());
//         self.buf = rest;
//
//         Ok(u64::from_be_bytes(buf.try_into()?))
//     }
//
//     pub fn next_entry(&mut self) -> Result<Option<ReserveEntry>> {
//         if self.done {
//             Ok(None)
//         } else {
//             let entry = {
//                 let address = self.read_u64()?;
//                 let size = self.read_u64()?;
//
//                 Ok(ReserveEntry { address, size })
//             };
//
//             self.done = entry.is_err()
//                 || entry
//                     .as_ref()
//                     .map(|e| e.address == 0 || e.size == 0)
//                     .unwrap_or_default();
//
//             entry.map(Some)
//         }
//     }
// }

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

    pub fn next(&mut self) -> Result<Option<&'dt str>> {
        if self.offset == self.bytes.len() || self.err {
            return Ok(None);
        }

        let str = read_str(&self.bytes, self.offset as u32)?;
        self.offset += str.len() + 1;

        Ok(Some(str))
    }
}
