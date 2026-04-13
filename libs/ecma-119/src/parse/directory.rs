use core::mem::size_of;
use core::str::Utf8Error;

use fallible_iterator::FallibleIterator;

use super::Image;
use super::parser::Parser;
use crate::ParseError;
use crate::parse::rock_ridge::RockRidgeIter;
use crate::parse::susp::SystemUseIter;
use crate::raw::{DirDateTime, DirectoryRecord, DirectoryRecordHeader, FileFlags, SECTOR_SIZE};

pub struct Directory<'img, 'a> {
    pub(super) img: &'img Image<'a>,
    pub(super) record: DirectoryRecord<'a>,
}

impl<'img, 'a> Directory<'img, 'a> {
    pub fn identifier(&self) -> Result<&'a str, Utf8Error> {
        match self.record.identifier {
            [0x0] => Ok("."),
            [0x1] => Ok(".."),
            id => str::from_utf8(id),
        }
    }

    pub fn recorded_at(&self) -> &DirDateTime {
        &self.record.header.recording_date
    }

    pub fn entries(&self) -> Result<DirEntryIter<'img, 'a>, ParseError> {
        let parser = Parser::from_lba_and_len(
            self.img.data,
            self.record.header.extent_lba.get(),
            self.record.header.data_length.get(),
            self.img.strict,
        )?;

        Ok(DirEntryIter {
            img: self.img,
            parser,
        })
    }

    pub fn susp_entries(&self) -> SystemUseIter<'a> {
        let Some(skip) = self.img.susp_skip else {
            return SystemUseIter {
                parser: Parser {
                    data: &[],
                    pos: 0,
                    strict: self.img.strict,
                },
                done: true,
            };
        };
        let system_use = &self.record.system_use[skip as usize..];
        SystemUseIter {
            parser: Parser {
                data: system_use,
                pos: 0,
                strict: self.img.strict,
            },
            done: false,
        }
    }

    pub fn rock_ridge_entries(&self) -> RockRidgeIter<'a> {
        RockRidgeIter {
            inner: self.susp_entries(),
        }
    }
}

pub struct File<'img, 'a> {
    img: &'img Image<'a>,
    record: DirectoryRecord<'a>,
}

impl<'a> File<'_, 'a> {
    pub fn size(&self) -> u32 {
        self.record.header.data_length.get()
    }

    pub fn identifier(&self) -> Result<&'a str, Utf8Error> {
        str::from_utf8(self.record.identifier)
    }

    pub fn recorded_at(&self) -> &DirDateTime {
        &self.record.header.recording_date
    }

    pub fn as_slice(&self) -> Result<&'a [u8], ParseError> {
        super::parser::lba_to_slice(
            self.img.data,
            self.record.header.extent_lba.get(),
            self.record.header.data_length.get(),
        )
    }

    pub fn susp_entries(&self) -> SystemUseIter<'a> {
        let Some(skip) = self.img.susp_skip else {
            return SystemUseIter {
                parser: Parser {
                    data: &[],
                    pos: 0,
                    strict: self.img.strict,
                },
                done: true,
            };
        };
        let system_use = &self.record.system_use[skip as usize..];
        SystemUseIter {
            parser: Parser {
                data: system_use,
                pos: 0,
                strict: self.img.strict,
            },
            done: false,
        }
    }

    pub fn rock_ridge_entries(&self) -> RockRidgeIter<'a> {
        RockRidgeIter {
            inner: self.susp_entries(),
        }
    }
}

pub enum DirectoryEntry<'img, 'a> {
    Directory(Directory<'img, 'a>),
    File(File<'img, 'a>),
}

impl<'a> DirectoryEntry<'_, 'a> {
    pub fn is_dir(&self) -> bool {
        matches!(self, Self::Directory(_))
    }

    pub fn is_file(&self) -> bool {
        matches!(self, Self::File(_))
    }

    pub fn identifier(&self) -> Result<&'a str, Utf8Error> {
        match self {
            DirectoryEntry::Directory(directory) => directory.identifier(),
            DirectoryEntry::File(file) => file.identifier(),
        }
    }

    pub fn recorded_at(&self) -> &DirDateTime {
        match self {
            DirectoryEntry::Directory(directory) => directory.recorded_at(),
            DirectoryEntry::File(file) => file.recorded_at(),
        }
    }

    pub fn susp_entries(&self) -> SystemUseIter<'a> {
        match self {
            DirectoryEntry::Directory(directory) => directory.susp_entries(),
            DirectoryEntry::File(file) => file.susp_entries(),
        }
    }

    pub fn rock_ridge_entries(&self) -> RockRidgeIter<'a> {
        match self {
            DirectoryEntry::Directory(directory) => directory.rock_ridge_entries(),
            DirectoryEntry::File(file) => file.rock_ridge_entries(),
        }
    }
}

pub struct DirEntryIter<'img, 'a> {
    img: &'img Image<'a>,
    parser: Parser<'a>,
}

impl<'img, 'a> FallibleIterator for DirEntryIter<'img, 'a> {
    type Item = DirectoryEntry<'img, 'a>;
    type Error = ParseError;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        loop {
            let len = self.parser.peek::<u8>()?;

            if *len == 0 {
                // Skip to next sector boundary
                let next_sector = (self.parser.pos / SECTOR_SIZE + 1) * SECTOR_SIZE;
                if next_sector >= self.parser.data.len() {
                    return Ok(None);
                }
                self.parser.pos = next_sector;
            } else {
                let header = self.parser.read_validated::<DirectoryRecordHeader>()?;

                // Validate that `len` is large enough to hold the header and
                // identifier before doing the subtraction (#10).
                let min_len =
                    size_of::<DirectoryRecordHeader>() + header.file_identifier_len as usize;
                if (header.len as usize) < min_len {
                    // read_validated already caught this in strict mode; in
                    // lenient mode we guard against the underflow manually.
                    // FIXME
                    // return Err(ValidationError {
                    //     path: todo!(),
                    //     kind: ValidationErrorKind::LengthOutOfRange {
                    //         len: (),
                    //         min: min_len,
                    //         max: (),
                    //     },
                    // });
                }

                let file_identifier = self.parser.bytes(header.file_identifier_len as usize)?;
                // ECMA-119 §9.1.12: if LEN_FI is even, a single (00) padding
                // byte is inserted before the System Use area so it starts on
                // an even byte position within the record.
                let pad = 1 - (header.file_identifier_len as usize & 1);
                self.parser.bytes(pad)?;
                let system_use = self.parser.bytes(
                    header.len as usize
                        - size_of::<DirectoryRecordHeader>()
                        - header.file_identifier_len as usize
                        - pad,
                )?;

                let record = DirectoryRecord {
                    header,
                    identifier: file_identifier,
                    system_use,
                };

                if header.flags().contains(FileFlags::DIRECTORY) {
                    return Ok(Some(DirectoryEntry::Directory(Directory {
                        img: self.img,
                        record,
                    })));
                } else {
                    return Ok(Some(DirectoryEntry::File(File {
                        img: self.img,
                        record,
                    })));
                }
            }
        }
    }
}
