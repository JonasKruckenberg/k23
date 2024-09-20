use core::ffi::c_void;
use core::{fmt, str};
use gimli::{EndianSlice, NativeEndian};
use object::read::elf::ElfFile64;
use object::{Object, ObjectSection};
use rustc_demangle::{try_demangle, Demangle};

pub enum Symbol<'a> {
    /// We were able to locate frame information for this symbol, and
    /// `addr2line`'s frame internally has all the nitty gritty details.
    Frame {
        addr: *mut c_void,
        location: Option<addr2line::Location<'a>>,
        name: Option<&'a str>,
    },
    /// Couldn't find debug information, but we found it in the symbol table of
    /// the elf executable.
    Symtab { name: &'a str },
}

impl<'a> Symbol<'a> {
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

impl<'a> fmt::Debug for Symbol<'a> {
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

impl<'a> fmt::Display for SymbolName<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref s) = self.demangled {
            return s.fmt(f);
        }

        f.write_str(self.raw)
    }
}

impl<'a> fmt::Debug for SymbolName<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref s) = self.demangled {
            return s.fmt(f);
        }

        f.write_str(self.raw)
    }
}

pub struct SymbolsIter<'a, 'ctx> {
    addr: u64,
    symtab: object::SymbolMap<object::SymbolMapName<'a>>,
    iter: addr2line::FrameIter<'ctx, EndianSlice<'a, NativeEndian>>,
    anything: bool,
}

impl<'a, 'ctx> SymbolsIter<'a, 'ctx> {
    fn search_symtab(&self) -> Option<&'a str> {
        Some(self.symtab.get(self.addr)?.name())
    }

    pub fn next(&mut self) -> gimli::Result<Option<Symbol<'ctx>>> {
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
    addr2line: addr2line::Context<EndianSlice<'a, NativeEndian>>,
    elf: ElfFile64<'a>,
    adjust_vma: u64,
}

impl<'a> SymbolizeContext<'a> {
    pub fn new(elf: ElfFile64<'a>, adjust_vma: u64) -> gimli::Result<Self> {
        let dwarf = gimli::Dwarf::load(|section_id| -> gimli::Result<_> {
            let data = match elf.section_by_name(section_id.name()) {
                Some(section) => section.data().unwrap(),
                None => &[],
            };
            Ok(EndianSlice::new(data, NativeEndian))
        })?;
        let addr2line = addr2line::Context::from_dwarf(dwarf)?;

        Ok(Self {
            addr2line,
            elf,
            adjust_vma,
        })
    }

    pub fn resolve_unsynchronized(&self, probe: u64) -> gimli::Result<SymbolsIter<'a, '_>> {
        let probe = probe - self.adjust_vma;
        let iter = self.addr2line.find_frames(probe).skip_all_loads()?;

        Ok(SymbolsIter {
            addr: probe,
            symtab: self.elf.symbol_map(),
            iter,
            anything: false,
        })
    }
}
