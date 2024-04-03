#![cfg_attr(not(test), no_std)]
#![feature(error_in_core, trait_upcasting)]

mod binary_reader;
mod code;
mod error;
mod indices;
mod instructions;
mod leb128;
mod limits;
mod module;
mod names;
mod types;
mod vec_reader;

pub use binary_reader::BinaryReader;
pub use code::{ConstExpr, FunctionBody, InstructionsIter, Locals};
pub use error::Error;
pub use indices::*;
pub use instructions::{BlockType, BrTable, Ieee32, Ieee64, Instruction, MemArg, V128};
pub use limits::*;
pub use module::Module;
pub use names::{IndirectNaming, NameSectionReader, NameSubsection, Naming};
pub use types::{
    FunctionType, GlobalType, Limits, MemoryType, Mutability, NumberType, ReferenceType, TableType,
    ValueType, VectorType,
};
pub use vec_reader::{VecIter, VecReader};

type Result<T> = core::result::Result<T, Error>;

pub enum Section<'a> {
    Custom(CustomSection<'a>),
    Type(VecReader<'a, FunctionType<'a>>),
    Import(VecReader<'a, Import<'a>>),
    Function(VecReader<'a, TypeIdx>),
    Table(VecReader<'a, TableType>),
    Memory(VecReader<'a, MemoryType>),
    Global(VecReader<'a, Global<'a>>),
    Export(VecReader<'a, Export<'a>>),
    Start(FuncIdx),
    Element(VecReader<'a, Element<'a>>),
    Code(VecReader<'a, FunctionBody<'a>>),
    Data(VecReader<'a, Data<'a>>),
    DataCount(u32),
}

pub struct CustomSection<'a> {
    pub name: &'a str,
    pub bytes: &'a [u8],
}

#[derive(Debug)]
pub struct Data<'a> {
    pub mode: DataMode<'a>,
    pub init: &'a [u8],
}

#[derive(Debug)]
pub enum DataMode<'a> {
    Passive,
    Active {
        mem: Option<MemIdx>,
        offset: ConstExpr<'a>,
    },
}

#[derive(Debug)]
pub struct Element<'a> {
    pub mode: ElementMode<'a>,
    pub items: ElementItems<'a>,
}

#[derive(Debug)]
pub enum ElementMode<'a> {
    Passive,
    Active {
        table: Option<TableIdx>,
        offset: ConstExpr<'a>,
    },
    Declarative,
}

#[derive(Debug)]
pub enum ElementItems<'a> {
    Functions(VecReader<'a, FuncIdx>),
    Expressions(ReferenceType, VecReader<'a, ConstExpr<'a>>),
}

#[derive(Debug)]
pub struct Export<'a> {
    pub name: &'a str,
    pub desc: ExportDesc,
}

#[derive(Debug)]
pub enum ExportDesc {
    Func(FuncIdx),
    Table(TableIdx),
    Mem(MemIdx),
    Global(GlobalIdx),
}

#[derive(Debug, Clone)]
pub struct Global<'a> {
    pub ty: GlobalType,
    pub init: ConstExpr<'a>,
}

pub struct Import<'a> {
    pub module: &'a str,
    pub name: &'a str,
    pub desc: ImportDesc,
}

pub enum ImportDesc {
    Func(FuncIdx),
    Table(TableType),
    Mem(MemoryType),
    Global(GlobalType),
}

pub fn parse_module(bytes: &[u8]) -> crate::Result<Module> {
    if !Module::is_wasm_module(bytes) {
        return Err(crate::Error::InvalidMagicNumber);
    }

    Ok(Module {
        reader: BinaryReader::new(&bytes[8..]),
    })
}
