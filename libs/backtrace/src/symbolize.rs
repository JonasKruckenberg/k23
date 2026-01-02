// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ffi::c_void;
use core::{fmt, str};

use fallible_iterator::FallibleIterator;
use gimli::{EndianSlice, NativeEndian};
use rustc_demangle::{Demangle, try_demangle};
use xmas_elf::sections::SectionData;
use xmas_elf::symbol_table::Entry;

pub enum Symbol<'a> {
    /// We were able to locate frame information for this symbol, and
    /// `addr2line`'s frame internally has all the nitty gritty details.
    Frame {
        addr: *mut c_void,
        location: Option<k23_addr2line::Location<'a>>,
        name: Option<&'a str>,
    },
    /// Couldn't find debug information, but we found it in the symbol table of
    /// the elf executable.
    Symtab { name: &'a str },
}

impl Symbol<'_> {
    pub fn name(&self) -> Option<SymbolName<'_>> {
        match self {
            Symbol::Frame { name, .. } => {
                let name = name.as_ref()?;
                Some(SymbolName::new(name))
            }
            Symbol::Symtab { name, .. } => Some(SymbolName::new(name)),
        }
    }

    pub fn addr(&self) -> Option<*mut c_void> {
        match self {
            Symbol::Frame { addr, .. } => Some(*addr),
            Symbol::Symtab { .. } => None,
        }
    }

    pub fn filename(&self) -> Option<&str> {
        match self {
            Symbol::Frame { location, .. } => {
                let file = location.as_ref()?.file?;
                Some(file)
            }
            Symbol::Symtab { .. } => None,
        }
    }

    pub fn lineno(&self) -> Option<u32> {
        match self {
            Symbol::Frame { location, .. } => location.as_ref()?.line,
            Symbol::Symtab { .. } => None,
        }
    }

    pub fn colno(&self) -> Option<u32> {
        match self {
            Symbol::Frame { location, .. } => location.as_ref()?.column,
            Symbol::Symtab { .. } => None,
        }
    }
}

impl fmt::Debug for Symbol<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Symbol");
        d.field("name", &self.name());
        d.field("addr", &self.addr());
        d.field("filename", &self.filename());
        d.field("lineno", &self.lineno());
        d.field("colno", &self.colno());
        d.finish()
    }
}

pub struct SymbolName<'a> {
    raw: &'a str,
    demangled: Option<Demangle<'a>>,
}

impl<'a> SymbolName<'a> {
    pub fn new(raw: &'a str) -> SymbolName<'a> {
        let demangled = try_demangle(raw).ok();

        Self { raw, demangled }
    }

    pub fn as_raw_str(&self) -> &'a str {
        self.demangled
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or(self.raw)
    }
}

impl fmt::Display for SymbolName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref s) = self.demangled {
            return s.fmt(f);
        }

        f.write_str(self.raw)
    }
}

impl fmt::Debug for SymbolName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref s) = self.demangled {
            return s.fmt(f);
        }

        f.write_str(self.raw)
    }
}

pub struct SymbolsIter<'a, 'ctx> {
    addr: u64,
    elf: &'ctx xmas_elf::ElfFile<'a>,
    symtab: &'ctx [xmas_elf::symbol_table::Entry64],
    iter: k23_addr2line::FrameIter<'ctx, EndianSlice<'a, NativeEndian>>,
    anything: bool,
}

impl<'ctx> SymbolsIter<'_, 'ctx> {
    fn search_symtab(&self) -> Option<&'ctx str> {
        self.symtab
            .iter()
            .find(|sym| sym.value() == self.addr)
            .map(|sym| sym.get_name(self.elf).unwrap())
    }
}

impl<'ctx> FallibleIterator for SymbolsIter<'_, 'ctx> {
    type Item = Symbol<'ctx>;
    type Error = gimli::Error;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(frame) = self.iter.next()? {
            self.anything = true;

            let name = if let Some(func) = frame.function {
                str::from_utf8(func.name.slice()).ok()
            } else {
                self.search_symtab()
            };

            Ok(Some(Symbol::Frame {
                addr: self.addr as *mut c_void,
                location: frame.location,
                name,
            }))
        } else if !self.anything {
            self.anything = true;
            // the iterator didn't produce any frames, so let's try the symbol table
            if let Some(name) = self.search_symtab() {
                Ok(Some(Symbol::Symtab { name }))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }
}

/// Context necessary to resolve an address to its symbol name and source location.
pub struct SymbolizeContext<'a> {
    addr2line: k23_addr2line::Context<EndianSlice<'a, NativeEndian>>,
    elf: xmas_elf::ElfFile<'a>,
    adjust_vma: u64,
}

impl<'a> SymbolizeContext<'a> {
    /// # Errors
    ///
    /// Returns an error when parsing the DWARF fails.
    pub fn new(elf: xmas_elf::ElfFile<'a>, adjust_vma: u64) -> gimli::Result<Self> {
        let dwarf = gimli::Dwarf::load(|section_id| -> gimli::Result<_> {
            let data = match elf.find_section_by_name(section_id.name()) {
                Some(section) => section.raw_data(&elf),
                None => &[],
            };
            Ok(EndianSlice::new(data, NativeEndian))
        })?;
        let addr2line = k23_addr2line::Context::from_dwarf(dwarf)?;

        Ok(Self {
            addr2line,
            elf,
            adjust_vma,
        })
    }

    /// # Errors
    ///
    /// Returns an error if the given address doesn't correspond to a symbol or parsing the DWARF info
    /// fails.
    ///
    /// # Panics
    ///
    /// Panics if the ELF file doesn't contain a symbol table.
    pub fn resolve_unsynchronized(&self, probe: u64) -> gimli::Result<SymbolsIter<'a, '_>> {
        let probe = probe - self.adjust_vma;
        let iter = self.addr2line.find_frames(probe).skip_all_loads()?;

        let symtab = self
            .elf
            .section_iter()
            .find_map(|section| {
                section
                    .get_data(&self.elf)
                    .ok()
                    .and_then(|data| match data {
                        SectionData::SymbolTable64(symtab) => Some(symtab),
                        _ => None,
                    })
            })
            .unwrap();

        Ok(SymbolsIter {
            addr: probe,
            elf: &self.elf,
            symtab,
            iter,
            anything: false,
        })
    }
}
