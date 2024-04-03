use crate::binary_reader::BinaryReader;
use crate::limits::{MAX_WASM_FUNCTION_PARAMS, MAX_WASM_FUNCTION_RETURNS};
use crate::vec_reader::VecReader;
use core::fmt;
use core::fmt::Formatter;

#[derive(Debug)]
pub enum NumberType {
    I32,
    I64,
    F32,
    F64,
}

#[derive(Debug)]
pub enum VectorType {
    V128,
}

#[derive(Debug, Clone)]
pub enum ReferenceType {
    FuncRef,
    ExternRef,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ValueType {
    I32,
    I64,
    F32,
    F64,
    V128,
    FuncRef,
    ExternRef,
}

#[derive(Clone)]
pub struct FunctionType<'a> {
    pub(crate) reader: BinaryReader<'a>,
    pub(crate) results_offset: usize,
}

#[derive(Debug, Clone)]
pub enum Limits {
    Unbounded(u32),
    Bounded(u32, u32),
}

#[derive(Debug, Clone)]
pub struct MemoryType {
    pub limits: Limits,
}

#[derive(Debug, Clone)]
pub struct TableType {
    pub ty: ReferenceType,
    pub limits: Limits,
}

#[derive(Debug, Clone)]
pub struct GlobalType {
    pub ty: ValueType,
    pub mutability: Mutability,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Mutability {
    Const,
    Var,
}

impl<'a> fmt::Debug for FunctionType<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("FunctionType")
            .field("params", &self.params())
            .field("results", &self.results())
            .finish()
    }
}

impl<'a> FunctionType<'a> {
    pub fn params(&self) -> crate::Result<VecReader<'a, ValueType>> {
        VecReader::new(
            self.reader.remaining_bytes(),
            BinaryReader::read_value_type,
            Some(MAX_WASM_FUNCTION_PARAMS),
        )
    }

    pub fn results(&self) -> crate::Result<VecReader<'a, ValueType>> {
        let bytes = &self.reader.remaining_bytes()[self.results_offset..];
        VecReader::new(
            bytes,
            BinaryReader::read_value_type,
            Some(MAX_WASM_FUNCTION_RETURNS),
        )
    }
}
