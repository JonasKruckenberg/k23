//! A parser for the device tree blob format.
//!
//! The device tree blob format is a binary format used by firmware to describe non-discoverable
//! hardware to the operating system. This includes things like the number of CPUs and their frequency,
//! MMIO regions, interrupt controllers, and other platform-specific information.
//!
//! The format is described in detail in the [Device Tree Specification](https://github.com/devicetree-org/devicetree-specification);
//!
//! In contrast to other DTB parsers where the entire DTB is traversed multiple times,
//! searching for sub-nodes and properties, this crate makes use of the [Visitor Pattern](https://rust-unofficial.github.io/patterns/patterns/behavioural/visitor.html)
//! to only traverse the DTB once, allowing the caller to collect information from the DTB in a single pass.
//! The visitor pattern also allows the caller to choose which nodes and properties they are interested in.
//!
//! # Example
//!
//! ```rust,no_run
//! use dtb_parser::{Node, Visit, Dtb};
//!
//! let dtb_ptr = 0x1234_5678 as *const u8; // get a pointer to the DTB
//! let dtb = unsafe { Dtb::from_raw(dtb_ptr) }.unwrap();
//!
//! struct MyVisitor {
//!    cpu_count: usize,
//! }
//!
//! impl<'a> Visit<'a> for MyVisitor {
//!   fn visit_subnode(&mut self, name: &'a str, mut node: Node<'a>) -> Result<(), dtb_parser::Error> {
//!     if name == "cpus" {
//!         node.walk(&mut self)?; // walk the cpus node, calling visit_subnode for each subnode
//!     } else if name.starts_with("cpu@") {
//!         self.cpu_count += 1;
//!     }
//!     
//!     Ok(())
//!   }
//! }
//! ```

#![no_std]
#![feature(error_in_core)]

mod error;
mod node;
mod parser;

use crate::parser::Parser;
use core::slice;

pub use error::Error;
pub use node::Node;

pub(crate) type Result<T> = core::result::Result<T, Error>;

const DTB_MAGIC: u32 = 0xD00DFEED;
const DTB_VERSION: u32 = 17;

#[allow(unused_variables)]
pub trait Visit<'a> {
    fn visit_subnode(&mut self, name: &'a str, node: Node<'a>) -> core::result::Result<(), Error> {
        Ok(())
    }

    fn visit_reg(&mut self, reg: &'a [u8]) -> core::result::Result<(), Error> {
        Ok(())
    }

    fn visit_address_cells(&mut self, cells: u32) -> core::result::Result<(), Error> {
        Ok(())
    }

    fn visit_size_cells(&mut self, cells: u32) -> core::result::Result<(), Error> {
        Ok(())
    }

    fn visit_property(
        &mut self,
        name: &'a str,
        value: &'a [u8],
    ) -> core::result::Result<(), Error> {
        Ok(())
    }
}

pub struct Dtb<'a> {
    header: &'a Header,
    memory_slice: &'a [u8],
    parser: Parser<'a>,
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
            memory_slice,
            parser: Parser::new(struct_slice, strings_slice, 0),
        })
    }

    pub fn walk(self, visitor: &mut dyn Visit<'a>) -> crate::Result<()> {
        self.parser.walk(visitor)
    }
}

fn c_strlen_on_slice(slice: &[u8]) -> usize {
    let mut end = slice;
    while !end.is_empty() && *end.first().unwrap_or(&0) != 0 {
        end = &end[1..];
    }

    end.as_ptr() as usize - slice.as_ptr() as usize
}
