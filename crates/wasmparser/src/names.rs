use crate::limits::{
    MAX_WASM_DATA_SEGMENTS, MAX_WASM_ELEMENT_SEGMENTS, MAX_WASM_FUNCTIONS,
    MAX_WASM_FUNCTION_LOCALS, MAX_WASM_GLOBALS, MAX_WASM_MEMORIES, MAX_WASM_TABLES,
};
use crate::{BinaryReader, VecReader};

/// Represents a name for an index from the names section.
#[derive(Debug, Copy, Clone)]
pub struct Naming<'a> {
    /// The index being named.
    pub index: u32,
    /// The name for the index.
    pub name: &'a str,
}

/// Represents an indirect name in the names custom section.
#[derive(Debug, Clone)]
pub struct IndirectNaming<'a> {
    /// The indirect index of the name.
    pub index: u32,
    /// The map of names within the `index` prior.
    pub names: VecReader<'a, Naming<'a>>,
}

#[derive(Debug)]
pub enum NameSubsection<'a> {
    Module(&'a str),
    Function(VecReader<'a, Naming<'a>>),
    Local(VecReader<'a, IndirectNaming<'a>>),
    Label(VecReader<'a, IndirectNaming<'a>>),
    Table(VecReader<'a, Naming<'a>>),
    Memory(VecReader<'a, Naming<'a>>),
    Global(VecReader<'a, Naming<'a>>),
    Elem(VecReader<'a, Naming<'a>>),
    Data(VecReader<'a, Naming<'a>>),
}

pub struct NameSectionReader<'a> {
    reader: BinaryReader<'a>,
}

impl<'a> NameSectionReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            reader: BinaryReader::new(bytes),
        }
    }

    pub fn subsections(&self) -> NameSubsectionsIter<'a> {
        NameSubsectionsIter {
            reader: self.reader.clone(),
            err: false,
        }
    }
}

pub struct NameSubsectionsIter<'a> {
    reader: BinaryReader<'a>,
    err: bool,
}

impl<'a> Iterator for NameSubsectionsIter<'a> {
    type Item = crate::Result<NameSubsection<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.err || self.reader.remaining_bytes().is_empty() {
            None
        } else {
            let res = self.reader.read_name_subsection();
            self.err = res.is_err();
            Some(res)
        }
    }
}

impl<'a> BinaryReader<'a> {
    pub fn read_name_map(&mut self) -> crate::Result<Naming<'a>> {
        let index = self.read_u32_leb128()?;
        let name = self.read_str()?;

        Ok(Naming { index, name })
    }

    pub fn read_indirect_map(&mut self) -> crate::Result<IndirectNaming<'a>> {
        let index = self.read_u32_leb128()?;
        let names = VecReader::new(
            self.remaining_bytes(),
            Self::read_name_map,
            Some(MAX_WASM_FUNCTION_LOCALS),
        )?;

        Ok(IndirectNaming { index, names })
    }

    pub fn read_name_subsection(&mut self) -> crate::Result<NameSubsection<'a>> {
        let section_id = self.read_u8()?;
        let len = self.read_u32_leb128()?;

        log::debug!("names subsection id {section_id:?} {len}");

        match section_id {
            0 => {
                let name = self.read_str()?;
                Ok(NameSubsection::Module(name))
            }
            1 => self.read_name_section_inner(
                len,
                MAX_WASM_FUNCTIONS,
                Self::read_name_map,
                NameSubsection::Function,
            ),
            2 => self.read_name_section_inner(
                len,
                MAX_WASM_FUNCTIONS,
                Self::read_indirect_map,
                NameSubsection::Local,
            ),
            3 => self.read_name_section_inner(
                len,
                MAX_WASM_FUNCTIONS,
                Self::read_indirect_map,
                NameSubsection::Label,
            ),
            5 => self.read_name_section_inner(
                len,
                MAX_WASM_TABLES,
                Self::read_name_map,
                NameSubsection::Table,
            ),
            6 => self.read_name_section_inner(
                len,
                MAX_WASM_MEMORIES,
                Self::read_name_map,
                NameSubsection::Memory,
            ),
            7 => self.read_name_section_inner(
                len,
                MAX_WASM_GLOBALS,
                Self::read_name_map,
                NameSubsection::Global,
            ),
            8 => self.read_name_section_inner(
                len,
                MAX_WASM_ELEMENT_SEGMENTS,
                Self::read_name_map,
                NameSubsection::Elem,
            ),
            9 => self.read_name_section_inner(
                len,
                MAX_WASM_DATA_SEGMENTS,
                Self::read_name_map,
                NameSubsection::Data,
            ),
            // 11 => self.read_name_section_inner(
            //     len,
            //     MAX_WASM_FUNCTIONS,
            //     Self::read_name_map,
            //     NameSubsection::Function,
            // ),
            _ => todo!(),
        }
    }

    fn read_name_section_inner<T>(
        &mut self,
        len: u32,
        limit: usize,
        ctor: fn(&mut BinaryReader<'a>) -> crate::Result<T>,
        variant: fn(VecReader<'a, T>) -> NameSubsection<'a>,
    ) -> crate::Result<NameSubsection<'a>> {
        let bytes = self.read_bytes(len as usize)?;

        let section_reader = VecReader::new(bytes, ctor, Some(limit))?;

        Ok(variant(section_reader))
    }
}
