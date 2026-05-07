// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::marker::PhantomData;

use fallible_iterator::FallibleIterator;
use zerocopy::ByteOrder;

use super::parser::Parser;
use crate::raw::{PathTableRecord, PathTableRecordHeader};

pub struct PathTableIter<'a, E> {
    pub(super) parser: Parser<'a>,
    pub(super) endianness: PhantomData<E>,
}

impl<'a, O: ByteOrder + 'a> FallibleIterator for PathTableIter<'a, O> {
    type Item = PathTableRecord<'a, O>;
    type Error = anyhow::Error;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        if self.parser.pos == self.parser.data.len() {
            return Ok(None);
        }

        let header = self.parser.read::<PathTableRecordHeader<O>>()?;
        let directory_id = self.parser.bytes(header.len as usize)?;

        // NB: the length of the record must always be even
        if !header.len.is_multiple_of(2) {
            let _padding = self.parser.byte_array::<1>()?;
        }

        Ok(Some(PathTableRecord {
            header,
            directory_id,
        }))
    }
}
