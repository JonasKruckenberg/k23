// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::error::Error;
use crate::{Header, Node};

#[derive(Clone)]
pub struct Parser<'dt> {
    stream: Stream<'dt>,
    pub strings: StringsBlock<'dt>,
    pub structs: StructsBlock<'dt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct BigEndianU32(pub(crate) u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct BigEndianToken(pub(crate) BigEndianU32);

pub(crate) struct Stream<'dt>(&'dt [u32]);

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct StringsBlock<'dt>(pub(crate) &'dt [u8]);

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct StructsBlock<'dt>(pub(crate) &'dt [u32]);

impl<'dt> Parser<'dt> {
    pub fn new(data: &'dt [u32], strings: StringsBlock<'dt>, structs: StructsBlock<'dt>) -> Self {
        Self {
            stream: Stream::new(data),
            strings,
            structs,
        }
    }

    pub fn data(&self) -> &'dt [u32] {
        self.stream.0
    }

    pub fn byte_data(&self) -> &'dt [u8] {
        // SAFETY: it is always valid to cast a `u32` to 4 `u8`s
        unsafe {
            core::slice::from_raw_parts(
                self.stream.0.as_ptr().cast::<u8>(),
                self.stream.0.len() * 4,
            )
        }
    }

    pub fn advance_token(&mut self) -> Result<BigEndianToken, Error> {
        loop {
            match BigEndianToken(
                self.stream
                    .advance()
                    .map(BigEndianU32)
                    .ok_or(Error::UnexpectedEof)?,
            ) {
                BigEndianToken::NOP => continue,
                token @ (BigEndianToken::BEGIN_NODE
                | BigEndianToken::END_NODE
                | BigEndianToken::PROP
                | BigEndianToken::END) => break Ok(token),
                t => break Err(Error::InvalidToken(t)),
            }
        }
    }

    pub(crate) fn peek_token(&self) -> Result<BigEndianToken, Error> {
        self.clone().advance_token()
    }

    pub fn advance_u32(&mut self) -> Result<BigEndianU32, Error> {
        self.stream
            .advance()
            .map(BigEndianU32)
            .ok_or(Error::UnexpectedEof)
    }

    pub fn advance_cstr(&mut self) -> Result<&'dt core::ffi::CStr, Error> {
        // SAFETY: It is safe to reinterpret the stream data to a smaller integer size
        let bytes = unsafe {
            core::slice::from_raw_parts(
                self.stream.0.as_ptr().cast::<u8>(),
                self.stream.0.len() * 4,
            )
        };
        let cstr = core::ffi::CStr::from_bytes_until_nul(bytes)?;

        // Round up to the next multiple of 4, if necessary
        let skip = ((cstr.to_bytes_with_nul().len() + 3) & !3) / 4;
        self.stream.skip_many(skip);

        Ok(cstr)
    }

    pub fn advance_aligned(&mut self, n: usize) {
        // Round up to the next multiple of 4, if necessary
        let skip = ((n + 3) & !3) / 4;
        self.stream.skip_many(skip);
    }

    pub fn parse_header(&mut self) -> Result<Header, Error> {
        let magic = self.advance_u32()?.to_ne();
        let total_size = self.advance_u32()?.to_ne();
        let struct_offset = self.advance_u32()?.to_ne();
        let strings_offset = self.advance_u32()?.to_ne();
        let memory_reserve_map_offset = self.advance_u32()?.to_ne();
        let version = self.advance_u32()?.to_ne();
        let last_compatible_version = self.advance_u32()?.to_ne();
        let boot_cpuid = self.advance_u32()?.to_ne();
        let strings_size = self.advance_u32()?.to_ne();
        let structs_size = self.advance_u32()?.to_ne();

        Ok(Header {
            magic,
            total_size,
            structs_offset: struct_offset,
            strings_offset,
            memory_reserve_map_offset,
            version,
            last_compatible_version,
            boot_cpuid,
            strings_size,
            structs_size,
        })
    }

    pub fn parse_root(&mut self) -> Result<Node<'dt>, Error> {
        match self.advance_token()? {
            BigEndianToken::BEGIN_NODE => {}
            t => return Err(Error::UnexpectedToken(t)),
        }

        let byte_data = self.byte_data();
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
            _ => return Err(Error::UnexpectedEof),
        }

        // advance past this nodes name
        let name = self.advance_cstr()?;

        let starting_data = self.data();

        Ok(Node {
            name,
            raw: &starting_data[..starting_data.len() - 1],
            strings: self.strings,
            structs: self.structs,
        })
    }

    pub fn parse_raw_property(&mut self) -> Result<(usize, &'dt [u8]), Error> {
        match self.advance_token()? {
            BigEndianToken::PROP => {
                // Properties are in the format: <data len> <name offset> <data...>
                let len = usize::try_from(self.advance_u32()?.to_ne())?;
                let name_offset = usize::try_from(self.advance_u32()?.to_ne())?;
                let data = self.byte_data().get(..len).ok_or(Error::UnexpectedEof)?;

                self.advance_aligned(data.len());

                Ok((name_offset, data))
            }
            t => Err(Error::UnexpectedToken(t)),
        }
    }
}

impl BigEndianU32 {
    pub const fn from_ne(n: u32) -> Self {
        Self(n.to_be())
    }

    pub const fn to_ne(self) -> u32 {
        u32::from_be(self.0)
    }
}

impl BigEndianToken {
    pub const BEGIN_NODE: Self = Self(BigEndianU32::from_ne(1));
    pub const END_NODE: Self = Self(BigEndianU32::from_ne(2));
    pub const PROP: Self = Self(BigEndianU32::from_ne(3));
    pub const NOP: Self = Self(BigEndianU32::from_ne(4));
    pub const END: Self = Self(BigEndianU32::from_ne(9));
}

impl<'a> Stream<'a> {
    #[inline(always)]
    pub(crate) fn new(data: &'a [u32]) -> Self {
        Self(data)
    }

    #[inline(always)]
    pub(crate) fn advance(&mut self) -> Option<u32> {
        let ret = *self.0.first()?;
        self.0 = self.0.get(1..)?;
        Some(ret)
    }

    pub(crate) fn skip_many(&mut self, n: usize) {
        self.0 = self.0.get(n..).unwrap_or_default();
    }
}

impl Clone for Stream<'_> {
    fn clone(&self) -> Self {
        Self(self.0)
    }
}

impl<'a> StringsBlock<'a> {
    pub fn offset_at(self, offset: usize) -> Result<&'a str, Error> {
        core::ffi::CStr::from_bytes_until_nul(self.0.get(offset..).ok_or(Error::UnexpectedEof)?)?
            .to_str()
            .map_err(Into::into)
    }
}
