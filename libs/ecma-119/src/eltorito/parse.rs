use core::cmp;

use fallible_iterator::FallibleIterator;
use zerocopy::IntoBytes;

use super::raw::{
    InitialEntry, SectionEntry, SectionEntryExtension, SectionHeaderEntry, ValidationEntry,
};
use crate::parse::parser::Parser;
use crate::{BootRecord, Image, ParseError, SECTOR_SIZE};

#[derive(Debug)]
pub enum CatalogEntry<'a> {
    Validation(&'a ValidationEntry),
    InitialEntry(&'a InitialEntry),
    Header(&'a SectionHeaderEntry),
    Entry(&'a SectionEntry),
    Extension(&'a SectionEntryExtension),
}

impl BootRecord {
    pub fn boot_catalog_entries<'a>(
        &self,
        img: &Image<'a>,
    ) -> Result<BootCatalogIter<'a>, ParseError> {
        let lba = self.boot_catalog.get();

        let parser = Parser::from_lba_and_len(
            img.data,
            lba,
            // NB: there is no explicit len given for boot catalog, but most fit within a single sector
            // we bound it here to a max of 6 sectors. That should be enough even for the biggest images.
            cmp::min(
                img.data.len() as u32 - (lba * SECTOR_SIZE as u32),
                6 * SECTOR_SIZE as u32,
            ),
            img.strict,
        )?;

        Ok(BootCatalogIter {
            parser,
            state: State::Validation,
        })
    }
}

pub struct BootCatalogIter<'a> {
    parser: Parser<'a>,
    state: State,
}

enum State {
    // next record is the validation header
    Validation,
    // next record is the initial entry
    InitialEntry,
    // next record is a section header
    SectionHeader,
    // next record is a section entry;
    // when remaining hits zero we switch back to the next section header (or stop)
    SectionEntries {
        remaining: u16,
        has_more_sections: bool,
    },
    // next record may be an extension for the current section entry
    Extension {
        remaining_entries: u16,
        has_more_sections: bool,
    },
    Done,
}

impl<'a> FallibleIterator for BootCatalogIter<'a> {
    type Item = CatalogEntry<'a>;
    type Error = ParseError;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        loop {
            match self.state {
                State::Validation => {
                    let entry = self.parser.read::<ValidationEntry>()?;

                    let computed_checksum = entry
                        .as_bytes()
                        .chunks(2)
                        .map(|raw| u16::from_le_bytes(raw.try_into().unwrap()))
                        .fold(0, u16::wrapping_add);

                    assert_eq!(entry.header_id, 1);
                    assert_eq!(entry.key, [0x55, 0xAA]);
                    assert_eq!(computed_checksum, 0);

                    self.state = State::InitialEntry;
                    return Ok(Some(CatalogEntry::Validation(entry)));
                }

                State::InitialEntry => {
                    let entry = self.parser.read::<InitialEntry>()?;

                    self.state = State::SectionHeader;
                    return Ok(Some(CatalogEntry::InitialEntry(entry)));
                }

                State::SectionHeader => {
                    let header = self.parser.read::<SectionHeaderEntry>()?;

                    if matches!(header.header_indicator, 0x90 | 0x91) {
                        self.state = State::SectionEntries {
                            remaining: header.entries.get(),
                            has_more_sections: header.header_indicator == 0x90,
                        };
                        return Ok(Some(CatalogEntry::Header(header)));
                    } else {
                        self.state = State::Done;
                        return Ok(None);
                    }
                }

                State::SectionEntries {
                    remaining: 0,
                    has_more_sections: false,
                } => {
                    self.state = State::Done;
                    return Ok(None);
                }

                State::SectionEntries {
                    remaining: 0,
                    has_more_sections: true,
                } => {
                    self.state = State::SectionHeader;
                    continue;
                }

                State::SectionEntries {
                    remaining,
                    has_more_sections,
                } => {
                    let entry = self.parser.read::<SectionEntry>()?;

                    self.state = State::Extension {
                        remaining_entries: remaining - 1,
                        has_more_sections,
                    };
                    return Ok(Some(CatalogEntry::Entry(entry)));
                }

                State::Extension {
                    remaining_entries,
                    has_more_sections,
                } => {
                    let ext_indicator = self.parser.peek::<u8>()?;

                    if *ext_indicator != 0x44 {
                        // Not an extension — send back to entries without consuming a count
                        self.state = State::SectionEntries {
                            remaining: remaining_entries,
                            has_more_sections,
                        };
                        continue;
                    }

                    let ext = self.parser.read::<SectionEntryExtension>()?;

                    let more_extensions = ext.bits & 0x20 != 0;

                    if !more_extensions {
                        self.state = State::SectionEntries {
                            remaining: remaining_entries,
                            has_more_sections,
                        };
                    }

                    return Ok(Some(CatalogEntry::Extension(ext)));
                }

                State::Done => return Ok(None),
            }
        }
    }
}
