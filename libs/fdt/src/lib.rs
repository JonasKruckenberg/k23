#![no_std]

use crate::parser::{BigEndianToken, BigEndianU32, Parser, StringsBlock, StructsBlock};
use core::ffi::CStr;
use core::mem::MaybeUninit;
use core::ops::ControlFlow;
use core::{fmt, iter, slice};
pub use error::Error;
use fallible_iterator::FallibleIterator;
use hashbrown::HashMap;

mod error;
mod parser;

pub struct Fdt<'dt> {
    _header: Header,
    parser: Parser<'dt>,
    root_properties: HashMap<&'dt str, NodeProperty<'dt>>,
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

impl Header {
    fn valid_magic(&self) -> bool {
        self.magic == 0xd00dfeed
    }
}

impl fmt::Debug for Fdt<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fdt")
            .field("header", &self._header)
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
        let mut parser = Parser::new(data, StringsBlock(&[]));
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

        if !header.valid_magic() {
            return Err(Error::BadMagic);
        } else if data.len() < (header.total_size / 4) as usize {
            return Err(Error::UnexpectedEndOfData);
        }

        let mut parser = Parser::new(structs.0, strings);

        match parser.advance_token()? {
            BigEndianToken::BEGIN_NODE => {}
            t => return Err(Error::UnexpectedToken(t)),
        }

        let byte_data = parser.byte_data();
        match byte_data
            .get(byte_data.len() - 4..)
            .map(<[u8; 4]>::try_from)
        {
            Some(Ok(data @ [_, _, _, _])) => {
                match BigEndianToken(BigEndianU32(u32::from_ne_bytes(data))) {
                    BigEndianToken::END => {}
                    t => return Err(Error::UnexpectedToken(t)),
                }
            }
            _ => return Err(Error::UnexpectedEndOfData),
        }

        // advance past this nodes name
        parser
            .advance_cstr()?
            .to_str()
            .map_err(Error::InvalidUtf8)?;

        let mut root_properties = HashMap::new();
        while parser.peek_token()? == BigEndianToken::PROP {
            let (name_offset, raw) = parser.parse_raw_property()?;
            let name = parser.strings().offset_at(name_offset)?;
            root_properties.insert(name, NodeProperty { raw });
        }

        #[allow(tail_expr_drop_order, reason = "")]
        Ok(Self {
            _header: header,
            parser,
            root_properties,
        })
    }

    /// Create a new FDT from a pointer to a u32 slice.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing the FDT fails.
    ///
    /// # Safety
    ///
    /// The caller has to ensure the pointer is valid and the entire FDT is accessible.
    pub unsafe fn from_ptr(ptr: *const u32) -> Result<Self, Error> {
        // Safety: the caller has to ensure the pointer is valid and the entire FDT is accessible
        unsafe {
            let tmp_header = slice::from_raw_parts(ptr, size_of::<Header>());
            let real_size = usize::try_from(
                Parser::new(tmp_header, StringsBlock(&[]))
                    .parse_header()?
                    .total_size,
            )?;

            Self::new(slice::from_raw_parts(ptr, real_size))
        }
    }

    pub fn cell_sizes(&self) -> CellSizes {
        CellSizes::from_props(&self.root_properties).unwrap_or_default()
    }

    /// Traverse the FDT tree and call the visitor function for each node.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing the FDT fails.
    #[inline]
    pub fn walk<F, R>(&self, mut f: F) -> Result<Option<R>, Error>
    where
        F: FnMut(NodePath<'_, 'dt>, Node<'dt>) -> ControlFlow<R>,
    {
        self.walk_inner(&mut f)
    }

    fn walk_inner<R>(
        &self,
        f: &mut dyn FnMut(NodePath<'_, 'dt>, Node<'dt>) -> ControlFlow<R>,
    ) -> Result<Option<R>, Error> {
        let mut parser = self.parser.clone();

        let mut parent_sizes: [MaybeUninit<CellSizes>; 16] = [
            MaybeUninit::new(self.cell_sizes()),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
            MaybeUninit::uninit(),
        ];
        let mut path: [&str; 16] = [
            "", "", "", "", "", "", "", "", "", "", "", "", "", "", "", "",
        ];
        let mut parent_index: usize = 0;

        loop {
            while let Ok(BigEndianToken::END_NODE) = parser.peek_token() {
                let _ = parser.advance_token();
                parent_index = parent_index.saturating_sub(1);
            }

            match parser.advance_token() {
                Ok(BigEndianToken::BEGIN_NODE) => parent_index += 1,
                Ok(BigEndianToken::END) | Err(Error::UnexpectedEndOfData) => return Ok(None),
                Ok(t) => return Err(Error::UnexpectedToken(t)),
                Err(e) => return Err(e),
            }

            // add the node name to the path
            let name = parser.advance_cstr()?;
            path[parent_index] = name.to_str().map_err(Error::InvalidUtf8)?;

            // the call to `visit_node` above might have consumed some properties already,
            // but we need to consume the rest of the properties before advancing to the next node
            let mut properties = HashMap::new();
            while parser.peek_token()? == BigEndianToken::PROP {
                let (name_offset, raw) = parser.parse_raw_property()?;
                let name = parser.strings().offset_at(name_offset)?;
                properties.insert(name, NodeProperty { raw });
            }

            parent_sizes[parent_index].write(CellSizes::from_props(&properties).unwrap_or_else(
                // Safety: the unwrap_or_else guarantees that the parent_sizes[parent_index] is initialized
                || unsafe { MaybeUninit::assume_init(parent_sizes[parent_index - 1]) },
            ));

            // Call the visitor
            if let Some(res) = f(
                NodePath {
                    components: &path[..parent_index + 1],
                },
                Node {
                    name,
                    properties,
                    // Safety: the unwrap_or_else guarantees that the parent_sizes[parent_index] is initialized
                    parent_cell_sizes: unsafe {
                        MaybeUninit::assume_init(parent_sizes[parent_index])
                    },
                },
            )
            .break_value()
            {
                return Ok(Some(res));
            }
        }
    }
}

pub struct Node<'dt> {
    name: &'dt CStr,
    properties: HashMap<&'dt str, NodeProperty<'dt>>,
    parent_cell_sizes: CellSizes,
}

#[derive(Debug)]
pub struct NodeName<'dt> {
    pub name: &'dt str,
    pub unit_address: Option<&'dt str>,
}

/// Generic node property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeProperty<'dt> {
    pub raw: &'dt [u8],
}

impl fmt::Debug for Node<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Node")
            .field("name", &self.name())
            .field("cell_sizes", &self.parent_cell_sizes)
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

    pub fn property(&self, name: &str) -> Option<&NodeProperty<'dt>> {
        self.properties.get(name)
    }

    pub fn properties(&self) -> hashbrown::hash_map::Iter<&'dt str, NodeProperty<'dt>> {
        self.properties.iter()
    }

    pub fn reg(&self) -> Option<Regs<'dt>> {
        Some(Regs {
            cell_sizes: self.parent_cell_sizes,
            encoded_array: self.properties.get("reg").map(|prop| prop.raw)?,
        })
    }

    pub fn interrupt_cells(&self) -> Option<usize> {
        self.properties
            .get("#interrupt-cells")
            .and_then(|prop| prop.as_usize().ok())
    }
}

impl<'dt> NodeProperty<'dt> {
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

#[derive(PartialEq, Eq, Hash)]
pub struct NodePath<'a, 'dt> {
    pub components: &'a [&'dt str],
}

impl<'a, 'dt> IntoIterator for NodePath<'a, 'dt> {
    type Item = &'dt str;
    type IntoIter = iter::Copied<slice::Iter<'a, &'dt str>>;

    fn into_iter(self) -> Self::IntoIter {
        self.components.iter().copied()
    }
}

impl fmt::Debug for NodePath<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"")?;

        let mut started = false;
        for c in self.components {
            if started {
                f.write_str("/")?;
                f.write_str(c)?;
            } else {
                started = true;
                f.write_str(c)?;
            }
        }
        f.write_str("\"")?;

        Ok(())
    }
}

impl NodePath<'_, '_> {
    pub fn ends_with(&self, base: &str) -> bool {
        base.trim_start_matches('/')
            .rsplit('/')
            .eq(self.components.iter().rev().copied())
    }

    pub fn starts_with(&self, base: &str) -> bool {
        base.trim_start_matches('/')
            .split('/')
            .zip(self.components.iter().skip(1).copied())
            .all(|(a, b)| a == b)
    }
}

/// The number of cells (big endian u32s) that addresses and sizes take
#[derive(Debug, Clone, Copy)]
pub struct CellSizes {
    /// Size of values representing an address
    pub address_cells: usize,
    /// Size of values representing a size
    pub size_cells: usize,
}

impl Default for CellSizes {
    fn default() -> Self {
        CellSizes {
            address_cells: 2,
            size_cells: 1,
        }
    }
}

impl CellSizes {
    fn from_props(props: &HashMap<&str, NodeProperty<'_>>) -> Option<Self> {
        if let (Some(address_cells), Some(size_cells)) =
            (props.get("#address-cells"), props.get("#size-cells"))
        {
            Some(CellSizes {
                address_cells: address_cells.as_usize().unwrap(),
                size_cells: size_cells.as_usize().unwrap(),
            })
        } else {
            None
        }
    }
}

pub struct Regs<'dt> {
    cell_sizes: CellSizes,
    encoded_array: &'dt [u8],
}

#[derive(Debug)]
pub struct RegEntry {
    pub starting_address: usize,
    pub size: Option<usize>,
}

impl FallibleIterator for Regs<'_> {
    type Item = RegEntry;
    type Error = Error;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        if self.encoded_array.is_empty() {
            return Ok(None);
        }

        let address_bytes = self.cell_sizes.address_cells * 4;
        let size_bytes = self.cell_sizes.size_cells * 4;

        let Some(encoded_address) = self.encoded_array.get(..address_bytes) else {
            return Ok(None);
        };
        let Some(encoded_len) = self
            .encoded_array
            .get(address_bytes..address_bytes + size_bytes)
        else {
            return Ok(None);
        };

        self.encoded_array = &self.encoded_array[address_bytes + size_bytes..];

        let starting_address = match self.cell_sizes.address_cells {
            1 => usize::try_from(u32::from_be_bytes(encoded_address.try_into()?))?,
            2 => usize::try_from(u64::from_be_bytes(encoded_address.try_into()?))?,
            _ => return Err(Error::InalidCellSize),
        };

        let size = match self.cell_sizes.size_cells {
            0 => None,
            1 => usize::try_from(u32::from_be_bytes(encoded_len.try_into()?)).ok(),
            2 => usize::try_from(u64::from_be_bytes(encoded_len.try_into()?)).ok(),
            _ => return Err(Error::InalidCellSize),
        };

        Ok(Some(RegEntry {
            starting_address,
            size,
        }))
    }
}
