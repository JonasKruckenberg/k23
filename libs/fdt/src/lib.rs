#![no_std]

mod error;
mod parser;

pub use crate::error::Error;
use crate::parser::{BigEndianToken, Parser, StringsBlock, StructsBlock};
use core::ffi::CStr;
use core::{fmt, slice};
use fallible_iterator::FallibleIterator;

const DTB_MAGIC: u32 = 0xD00D_FEED;

pub struct Fdt<'dt> {
    header: Header,
    root: Node<'dt>,
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Header {
    /// FDT header magic
    pub magic: u32,
    /// Total size in bytes of the FDT structure
    pub total_size: u32,
    /// Offset in bytes from the start of the header to the structure block
    pub structs_offset: u32,
    /// Offset in bytes from the start of the header to the strings block
    pub strings_offset: u32,
    /// Offset in bytes from the start of the header to the memory reservation
    /// block
    pub memory_reserve_map_offset: u32,
    /// FDT version
    pub version: u32,
    /// Last compatible FDT version
    pub last_compatible_version: u32,
    /// System boot CPU ID
    pub boot_cpuid: u32,
    /// Length in bytes of the strings block
    pub strings_size: u32,
    /// Length in bytes of the struct block
    pub structs_size: u32,
}

pub struct Node<'dt> {
    name: &'dt CStr,
    raw: &'dt [u32],
    // parent: Option<&'dt [u32]>,
    strings: StringsBlock<'dt>,
    structs: StructsBlock<'dt>,
}

#[derive(Debug)]
pub struct NodeName<'dt> {
    pub name: &'dt str,
    pub unit_address: Option<&'dt str>,
}

#[derive(Debug)]
pub struct Property<'dt> {
    pub name: &'dt str,
    pub raw: &'dt [u8],
}

impl fmt::Debug for Fdt<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fdt")
            .field("header", &self.header)
            .finish_non_exhaustive()
    }
}

impl<'dt> Fdt<'dt> {
    /// Create a new FDT from a u32 slice.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing the FDT fails.
    pub fn new(data: &'dt [u32]) -> Result<Self, Error> {
        let mut parser = Parser::new(data, StringsBlock(&[]), StructsBlock(&[]));
        let header = parser.parse_header()?;

        let strings_end = (header.strings_offset + header.strings_size) as usize / 4;
        let structs_end = (header.structs_offset + header.structs_size) as usize / 4;
        if data.len() < strings_end || data.len() < structs_end {
            return Err(Error::SliceTooSmall);
        }

        let strings_start = header.strings_offset as usize;
        let strings_end = strings_start + header.strings_size as usize;

        // Safety: we have to trust the FDT header that the strings block is valid
        let strings = StringsBlock(unsafe {
            slice::from_raw_parts(data.as_ptr().cast(), size_of_val(data))
                .get(strings_start..strings_end)
                .ok_or(Error::UnexpectedEndOfData)?
        });

        let structs_start = header.structs_offset as usize / 4;
        let structs_end = structs_start + (header.structs_size as usize / 4);
        let structs = StructsBlock(
            data.get(structs_start..structs_end)
                .ok_or(Error::UnexpectedEndOfData)?,
        );

        if header.magic != DTB_MAGIC {
            return Err(Error::BadMagic);
        } else if data.len() < (header.total_size / 4) as usize {
            return Err(Error::UnexpectedEndOfData);
        }

        Ok(Self {
            header,
            root: Parser::new(structs.0, strings, structs).parse_root()?,
        })
    }

    /// Returns an iterator over all nodes in the tree.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing the FDT fails.
    pub fn nodes(&self) -> Result<NodesIter<'dt>, Error> {
        let mut parser = Parser::new(self.root.raw, self.root.strings, self.root.structs);

        while parser.peek_token()? == BigEndianToken::PROP {
            parser.parse_raw_property()?;
        }

        Ok(NodesIter { parser, depth: 0 })
    }

    pub fn properties(&self) -> PropertiesIter<'dt> {
        self.root.properties()
    }
}

impl fmt::Debug for Node<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Node")
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}

impl<'dt> Node<'dt> {
    /// Returns the name of the node.
    ///
    /// # Errors
    ///
    /// Returns an error if the name is not a valid UTF-8 string.
    pub fn name(&self) -> Result<NodeName<'dt>, Error> {
        self.name.to_str().map_err(Error::InvalidUtf8).map(|s| {
            if s.is_empty() {
                NodeName {
                    name: "/",
                    unit_address: None,
                }
            } else {
                let (name, unit_address) = s.split_once('@').unzip();
                NodeName {
                    name: name.unwrap_or(s),
                    unit_address,
                }
            }
        })
    }

    pub fn properties(&self) -> PropertiesIter<'dt> {
        PropertiesIter {
            parser: Parser::new(self.raw, self.strings, self.structs),
        }
    }
}

impl<'dt> Property<'dt> {
    /// Returns the property as a `u32`.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a u32.
    pub fn as_u32(&self) -> Result<u32, Error> {
        match self.raw {
            [a, b, c, d] => Ok(u32::from_be_bytes([*a, *b, *c, *d])),
            _ => Err(Error::InvalidPropertyValue),
        }
    }

    /// Returns the property as a `u64`.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a u64.
    pub fn as_u64(&self) -> Result<u64, Error> {
        match self.raw {
            [a, b, c, d] => Ok(u64::from_be_bytes([0, 0, 0, 0, *a, *b, *c, *d])),
            [a, b, c, d, e, f, g, h] => Ok(u64::from_be_bytes([*a, *b, *c, *d, *e, *f, *g, *h])),
            _ => Err(Error::InvalidPropertyValue),
        }
    }

    /// Returns the property as a `usize`.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a usize.
    pub fn as_usize(&self) -> Result<usize, Error> {
        #[cfg(target_pointer_width = "32")]
        let ret = match self.raw {
            [a, b, c, d] => Ok(usize::from_be_bytes([*a, *b, *c, *d])),
            _ => Err(Error::InvalidPropertyValue),
        };

        #[cfg(target_pointer_width = "64")]
        let ret = match self.raw {
            [a, b, c, d] => Ok(usize::from_be_bytes([0, 0, 0, 0, *a, *b, *c, *d])),
            [a, b, c, d, e, f, g, h] => Ok(usize::from_be_bytes([*a, *b, *c, *d, *e, *f, *g, *h])),
            _ => Err(Error::InvalidPropertyValue),
        };

        ret
    }

    /// Returns the property as a C string.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a valid C string.
    pub fn as_cstr(&self) -> Result<&'dt CStr, Error> {
        CStr::from_bytes_until_nul(self.raw).map_err(Into::into)
    }

    /// Returns the property as a string.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a valid UTF-8 string.
    pub fn as_str(&self) -> Result<&'dt str, Error> {
        core::str::from_utf8(self.raw)
            .map(|s| s.trim_end_matches('\0'))
            .map_err(Into::into)
    }

    /// Returns a fallible iterator over the strings in the property.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a valid UTF-8 string.
    pub fn as_strlist(&self) -> Result<StringList<'dt>, Error> {
        Ok(StringList {
            strs: self.as_str()?.split('\0'),
        })
    }
}

#[derive(Debug, Clone)]
pub struct StringList<'dt> {
    strs: core::str::Split<'dt, char>,
}

impl<'dt> Iterator for StringList<'dt> {
    type Item = &'dt str;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        self.strs.next()
    }
}

pub struct NodesIter<'dt> {
    pub(crate) parser: Parser<'dt>,
    pub(crate) depth: usize,
}
impl<'dt> FallibleIterator for NodesIter<'dt> {
    type Item = (usize, Node<'dt>);
    type Error = Error;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        while let Ok(BigEndianToken::END_NODE) = self.parser.peek_token() {
            let _ = self.parser.advance_token();
            self.depth = self.depth.saturating_sub(1);
        }

        match self.parser.advance_token() {
            Ok(BigEndianToken::BEGIN_NODE) => self.depth += 1,
            Ok(BigEndianToken::END) | Err(Error::UnexpectedEndOfData) => return Ok(None),
            Ok(t) => return Err(Error::UnexpectedToken(t)),
            Err(e) => return Err(e),
        }

        let name = self.parser.advance_cstr()?;
        let starting_data = self.parser.data();

        while self.parser.peek_token()? == BigEndianToken::PROP {
            self.parser.parse_raw_property()?;
        }

        Ok(Some((
            self.depth,
            Node {
                name,
                raw: starting_data,
                strings: self.parser.strings,
                structs: self.parser.structs,
            },
        )))
    }
}

pub struct PropertiesIter<'dt> {
    pub(crate) parser: Parser<'dt>,
}
impl<'dt> FallibleIterator for PropertiesIter<'dt> {
    type Item = Property<'dt>;
    type Error = Error;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        if self.parser.peek_token()? == BigEndianToken::PROP {
            let (name_offset, raw) = self.parser.parse_raw_property()?;
            let name = self.parser.strings.offset_at(name_offset)?;

            Ok(Some(Property { name, raw }))
        } else {
            Ok(None)
        }
    }
}
