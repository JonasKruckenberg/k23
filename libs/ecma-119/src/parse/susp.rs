// - the directory record system use area is divided into variable len fields "system use entries"
// - each entry is identified by a system use entry signature word
// - more than one entry of the same type may exist (unless explicitly forbidden)
// - entries are unordered

use core::mem::size_of;

use fallible_iterator::FallibleIterator;

use crate::parse::parser::Parser;
use crate::{
    ParseError, SystemUseEntry, SystemUseEntryCE, SystemUseEntryER, SystemUseEntryERHeader,
    SystemUseEntryES, SystemUseEntryHeader, SystemUseEntrySP,
};

pub struct SystemUseIter<'a> {
    pub(super) parser: Parser<'a>,
    pub(super) done: bool,
}

impl<'a> FallibleIterator for SystemUseIter<'a> {
    type Item = SystemUseEntry<'a>;
    type Error = ParseError;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        loop {
            if self.done {
                return Ok(None);
            }
            // SUSP areas may be zero-padded; once there isn't room for another
            // header the iteration is implicitly over.
            if self.parser.pos + size_of::<SystemUseEntryHeader>() > self.parser.data.len() {
                return Ok(None);
            }

            let header = self.parser.peek::<SystemUseEntryHeader>()?;
            let data_len = (header.len as usize)
                .checked_sub(size_of::<SystemUseEntryHeader>())
                .unwrap();

            let entry = match &header.signature {
                b"CE" => SystemUseEntry::ContinuationArea(
                    self.parser.read_validated::<SystemUseEntryCE>()?,
                ),
                b"SP" => {
                    SystemUseEntry::SuspIndicator(self.parser.read_validated::<SystemUseEntrySP>()?)
                }
                b"ES" => SystemUseEntry::ExtensionSelector(
                    self.parser.read_validated::<SystemUseEntryES>()?,
                ),
                b"ST" => {
                    self.parser.read::<SystemUseEntryHeader>()?;
                    self.done = true;
                    return Ok(None);
                }
                b"PD" => {
                    // Padding
                    self.parser.read::<SystemUseEntryHeader>()?;
                    self.parser.bytes(data_len)?;
                    continue;
                }
                b"ER" => {
                    let er_header = self.parser.read_validated::<SystemUseEntryERHeader>()?;
                    let identifier = self.parser.bytes(er_header.identifier_len as usize)?;
                    let descriptor = self.parser.bytes(er_header.descriptor_len as usize)?;
                    let source = self.parser.bytes(er_header.source_len as usize)?;
                    SystemUseEntry::ExtensionsReference(SystemUseEntryER {
                        header: er_header,
                        identifier,
                        descriptor,
                        source,
                    })
                }

                _ => {
                    let header = self.parser.read::<SystemUseEntryHeader>()?;
                    let data = self.parser.bytes(data_len)?;
                    SystemUseEntry::Unknown { header, data }
                }
            };

            return Ok(Some(entry));
        }
    }
}
