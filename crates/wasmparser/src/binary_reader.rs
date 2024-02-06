use crate::code::{ConstExpr, FunctionBody};
use crate::instructions::{BlockType, BrTable, Ieee32, Ieee64, Instruction, MemArg};
use crate::limits::{
    MAX_WASM_DATA_SEGMENTS, MAX_WASM_ELEMENT_SEGMENTS, MAX_WASM_EXPORTS, MAX_WASM_FUNCTIONS,
    MAX_WASM_GLOBALS, MAX_WASM_MEMORIES, MAX_WASM_STRING_SIZE, MAX_WASM_TABLES,
};
use crate::types::{
    FunctionType, GlobalType, Limits, MemoryType, Mutability, ReferenceType, TableType, ValueType,
};
use crate::vec_reader::VecReader;
use crate::{leb128, CustomSection, Section};
use crate::{Data, DataMode};
use crate::{Element, ElementItems, ElementMode};
use crate::{Error, TypeIdx};
use crate::{Export, ExportDesc};
use crate::{Global, MemIdx};
use crate::{Import, ImportDesc};
use core::str;

const CUSTOM_SECTION: u8 = 0;
const TYPE_SECTION: u8 = 1;
const IMPORT_SECTION: u8 = 2;
const FUNCTION_SECTION: u8 = 3;
const TABLE_SECTION: u8 = 4;
const MEMORY_SECTION: u8 = 5;
const GLOBAL_SECTION: u8 = 6;
const EXPORT_SECTION: u8 = 7;
const START_SECTION: u8 = 8;
const ELEMENT_SECTION: u8 = 9;
const CODE_SECTION: u8 = 10;
const DATA_SECTION: u8 = 11;
const DATA_COUNT_SECTION: u8 = 12;

#[derive(Clone)]
pub struct BinaryReader<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) pos: usize,
}

impl<'a> BinaryReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub fn ensure_bytes(&self, len: usize) -> crate::Result<()> {
        if self.pos + len > self.bytes.len() {
            Err(Error::UnexpectedEof)
        } else {
            Ok(())
        }
    }

    pub fn remaining_bytes(&self) -> &'a [u8] {
        &self.bytes[self.pos..]
    }

    pub fn read_u8(&mut self) -> crate::Result<u8> {
        self.ensure_bytes(1)?;
        let byte = self.bytes[self.pos];
        self.pos += 1;
        Ok(byte)
    }

    pub fn peek_u8(&self) -> crate::Result<u8> {
        self.ensure_bytes(1)?;
        let byte = self.bytes[self.pos];
        Ok(byte)
    }

    leb128::impl_read_unsigned_leb128!(read_u32_leb128, u32);
    leb128::impl_read_unsigned_leb128!(read_u64_leb128, u64);

    leb128::impl_read_signed_leb128!(read_i32_leb128, i32);
    leb128::impl_read_signed_leb128!(read_i64_leb128, i64);

    // special 33-bit signed int used in block types
    pub fn read_i33_leb128(&mut self) -> crate::Result<i64> {
        let mut result = 0;
        let mut shift = 0;
        let mut byte;

        loop {
            byte = self.read_u8()?;
            result |= i64::from(byte & 0x7F) << shift;
            shift += 7;

            if (byte & 0x80) == 0 {
                break;
            }
        }

        if (shift < 32) && ((byte & 0x40) != 0) {
            // sign extend
            result |= !0 << shift;
        }

        Ok(result)
    }

    // used in f32.const instrs
    pub fn read_f32(&mut self) -> crate::Result<Ieee32> {
        Ok(Ieee32(self.read_u32_leb128()?))
    }

    // used in f64.const instrs
    pub fn read_f64(&mut self) -> crate::Result<Ieee64> {
        Ok(Ieee64(self.read_u64_leb128()?))
    }

    pub fn read_bytes(&mut self, len: usize) -> crate::Result<&'a [u8]> {
        self.ensure_bytes(len)?;
        let bytes = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(bytes)
    }

    pub fn read_str(&mut self) -> crate::Result<&'a str> {
        let len = self.read_u32_leb128()? as usize;

        if len > MAX_WASM_STRING_SIZE {
            return Err(Error::StringTooLong);
        }

        let bytes = self.read_bytes(len)?;
        Ok(str::from_utf8(bytes)?)
    }

    pub fn read_const_expr(&mut self) -> crate::Result<ConstExpr<'a>> {
        let bytes = self.remaining_bytes();

        loop {
            if let Instruction::End = self.read_instruction()? {
                return Ok(ConstExpr {
                    reader: BinaryReader::new(bytes),
                });
            }
        }
    }

    pub fn read_function_body(&mut self) -> crate::Result<FunctionBody<'a>> {
        let size = self.read_u32_leb128()?;
        let bytes = self.read_bytes(size as usize)?;

        Ok(FunctionBody {
            reader: BinaryReader::new(bytes),
        })
    }

    pub fn skip_locals(&mut self) -> crate::Result<()> {
        let count = self.read_u32_leb128()?;
        for _ in 0..count {
            self.read_u32_leb128()?;
            self.read_value_type()?;
        }
        Ok(())
    }

    pub fn read_data(&mut self) -> crate::Result<Data<'a>> {
        let flags = self.read_u32_leb128()?;

        let mode = if flags & 0b01 != 0 {
            DataMode::Passive
        } else {
            let mem = if flags & 0b10 != 0 {
                Some(self.read_mem_idx()?)
            } else {
                None
            };

            let offset = self.read_const_expr()?;

            DataMode::Active { mem, offset }
        };

        let len = self.read_u32_leb128()?;
        let init = self.read_bytes(len as usize)?;

        Ok(Data { mode, init })
    }

    pub fn read_element(&mut self) -> crate::Result<Element<'a>> {
        let flags = self.read_u32_leb128()?;

        // bit 0 differentiates between active and declarative/passive
        let mode = if flags & 0b001 != 0 {
            // bit 1 differentiates between declarative and passive
            if flags & 0b010 != 0 {
                ElementMode::Declarative
            } else {
                ElementMode::Passive
            }
        } else {
            // bit indicates the presence of an explicit table index
            let table = if flags & 0b010 != 0 {
                Some(self.read_table_idx()?)
            } else {
                None
            };

            let offset = self.read_const_expr()?;
            // let offset = ConstExpr::new(r.remaining_bytes(), r.position());
            // r.skip_const_expr()?;

            ElementMode::Active { table, offset }
        };

        // bit 2 indicates element type + element expressions are used
        let uses_exprs = flags & 0b100 != 0;

        // variant 0 and 4 don't have an explicit type
        let ty = if flags & 0b011 != 0 {
            // 1,2,3,5,6,7
            if uses_exprs {
                // 5,6,7
                Some(self.read_reference_type()?)
            } else {
                // 1,2,3
                let b = self.read_u8()?;
                if b == 0 {
                    None
                } else {
                    return Err(Error::UnsupportedExternalInElement);
                }
            }
        } else {
            None
        };

        // TODO advance past the items
        let buf = self.remaining_bytes();

        let items_count = self.read_u32_leb128()?;
        for _ in 0..items_count {
            if uses_exprs {
                self.read_const_expr()?;
            } else {
                self.read_func_idx()?;
            }
        }

        let items = if uses_exprs {
            ElementItems::Expressions(
                ty.unwrap_or(ReferenceType::FuncRef),
                VecReader::new(buf, Self::read_const_expr, None)?,
            )
        } else {
            assert!(ty.is_none());
            ElementItems::Functions(VecReader::new(buf, Self::read_func_idx, None)?)
        };

        Ok(Element { mode, items })
    }

    pub fn read_export(&mut self) -> crate::Result<Export<'a>> {
        let name = self.read_str()?;
        let tag = self.read_u8()?;

        let desc = match tag {
            0x00 => ExportDesc::Func(self.read_func_idx()?),
            0x01 => ExportDesc::Table(self.read_table_idx()?),
            0x02 => ExportDesc::Mem(self.read_mem_idx()?),
            0x03 => ExportDesc::Global(self.read_global_idx()?),
            _ => return Err(Error::UnknownExportDescription(tag)),
        };

        Ok(Export { name, desc })
    }

    pub fn read_global(&mut self) -> crate::Result<Global<'a>> {
        let ty = self.read_global_type()?;
        let init = self.read_const_expr()?;

        Ok(Global { ty, init })
    }

    pub fn read_block_type(&mut self) -> crate::Result<BlockType> {
        let b = self.peek_u8()?;

        if b == 0x40 {
            self.read_u8()?;
            return Ok(BlockType::Empty);
        }

        if b <= 0x7F && b >= 0x6F {
            return Ok(BlockType::Type(self.read_value_type()?));
        }

        let idx = self.read_i33_leb128()?;
        match u32::try_from(idx) {
            Ok(idx) => Ok(BlockType::FunctionType(TypeIdx::from_u32(idx))),
            Err(_) => Err(Error::UnknownFunctionType),
        }
    }

    pub fn read_import(&mut self) -> crate::Result<Import<'a>> {
        let module = self.read_str()?;
        let name = self.read_str()?;
        let tag = self.read_u8()?;

        let desc = match tag {
            0x00 => ImportDesc::Func(self.read_func_idx()?),
            0x01 => ImportDesc::Table(self.read_table_type()?),
            0x02 => ImportDesc::Mem(self.read_memory_type()?),
            0x03 => ImportDesc::Global(self.read_global_type()?),
            _ => return Err(Error::UnknownImportDescription(tag)),
        };

        Ok(Import { module, name, desc })
    }

    pub fn read_reference_type(&mut self) -> crate::Result<ReferenceType> {
        let tag = self.read_u8()?;

        match tag {
            0x70 => Ok(ReferenceType::FuncRef),
            0x6F => Ok(ReferenceType::ExternRef),
            t => Err(Error::UnknownRefType(t)),
        }
    }

    pub fn read_value_type(&mut self) -> crate::Result<ValueType> {
        let tag = self.read_u8()?;

        match tag {
            0x7F => Ok(ValueType::I32),
            0x7E => Ok(ValueType::I64),
            0x7D => Ok(ValueType::F32),
            0x7C => Ok(ValueType::F64),
            0x7B => Ok(ValueType::V128),
            0x70 => Ok(ValueType::FuncRef),
            0x6F => Ok(ValueType::ExternRef),
            t => Err(Error::UnknownValType(t)),
        }
    }

    pub fn read_limits(&mut self) -> crate::Result<Limits> {
        let tag = self.read_u8()?;

        match tag {
            0x00 => Ok(Limits::Unbounded(self.read_u32_leb128()?)),
            0x01 => Ok(Limits::Bounded(
                self.read_u32_leb128()?,
                self.read_u32_leb128()?,
            )),
            t => Err(Error::UnknownLimit(t)),
        }
    }

    pub fn read_table_type(&mut self) -> crate::Result<TableType> {
        Ok(TableType {
            ty: self.read_reference_type()?,
            limits: self.read_limits()?,
        })
    }

    pub fn read_memory_type(&mut self) -> crate::Result<MemoryType> {
        Ok(MemoryType {
            limits: self.read_limits()?,
        })
    }

    pub fn read_global_type(&mut self) -> crate::Result<GlobalType> {
        let ty = self.read_value_type()?;
        let mutability = match self.read_u8()? {
            0x00 => Mutability::Const,
            0x01 => Mutability::Var,
            t => return Err(Error::UnknownGlobalMutability(t)),
        };

        Ok(GlobalType { ty, mutability })
    }

    pub fn read_function_type(&mut self) -> crate::Result<FunctionType<'a>> {
        let tag = self.read_u8()?;
        if tag != 0x60 {
            return Err(Error::InvalidFunctionType);
        }

        let buf = self.remaining_bytes();
        let params_offset = self.pos;
        let len_params = self.read_u32_leb128()?;
        for _ in 0..len_params {
            self.read_value_type()?;
        }

        let results_offset = self.pos;
        let len_results = self.read_u32_leb128()?;
        for _ in 0..len_results {
            self.read_value_type()?;
        }

        Ok(FunctionType {
            reader: BinaryReader::new(buf),
            results_offset: results_offset - params_offset,
        })
    }

    pub fn read_br_table(&mut self) -> crate::Result<BrTable<'a>> {
        let bytes = self.remaining_bytes();
        let len = self.read_u32_leb128()?;
        for _ in 0..len {
            self.read_label_idx()?;
        }
        let default = self.read_label_idx()?;

        Ok(BrTable {
            labels: VecReader::new(bytes, Self::read_label_idx, None)?,
            default,
        })
    }

    pub fn read_memarg(&mut self) -> crate::Result<MemArg> {
        let mut flags = self.read_u32_leb128()?;
        let memory = if flags & (1 << 6) != 0 {
            flags ^= 1 << 6;
            self.read_mem_idx()?
        } else {
            MemIdx::from_u32(0)
        };
        let align = if flags >= (1 << 6) {
            return Err(Error::AlignmentTooLarge);
        } else {
            flags as u8
        };

        Ok(MemArg {
            memory,
            align,
            offset: self.read_u64_leb128()?,
        })
    }

    pub fn read_section(&mut self) -> crate::Result<Section<'a>> {
        let section_id = self.read_u8()?;
        let len = self.read_u32_leb128()?;

        match section_id {
            CUSTOM_SECTION => {
                log::debug!("Parsing custom section... len {len:#x?}");
                let mut reader = BinaryReader::new(self.read_bytes(len as usize)?);

                let name = reader.read_str()?;
                let bytes = reader.remaining_bytes();

                Ok(Section::Custom(CustomSection { name, bytes }))
            }
            TYPE_SECTION => {
                log::debug!("Parsing type section...");
                self.read_section_inner(
                    len,
                    MAX_WASM_FUNCTIONS,
                    Self::read_function_type,
                    Section::Type,
                )
            }
            IMPORT_SECTION => {
                log::debug!("Parsing import section...");
                self.read_section_inner(len, MAX_WASM_EXPORTS, Self::read_import, Section::Import)
            }
            FUNCTION_SECTION => {
                log::debug!("Parsing function section...");
                self.read_section_inner(
                    len,
                    MAX_WASM_FUNCTIONS,
                    Self::read_type_idx,
                    Section::Function,
                )
            }
            TABLE_SECTION => {
                log::debug!("Parsing table section...");
                self.read_section_inner(len, MAX_WASM_TABLES, Self::read_table_type, Section::Table)
            }
            MEMORY_SECTION => {
                log::debug!("Parsing memory section...");
                self.read_section_inner(
                    len,
                    MAX_WASM_MEMORIES,
                    Self::read_memory_type,
                    Section::Memory,
                )
            }
            GLOBAL_SECTION => {
                log::debug!("Parsing global section...");
                self.read_section_inner(len, MAX_WASM_GLOBALS, Self::read_global, Section::Global)
            }
            EXPORT_SECTION => {
                log::debug!("Parsing export section...");
                self.read_section_inner(len, MAX_WASM_EXPORTS, Self::read_export, Section::Export)
            }
            START_SECTION => Ok(Section::Start(self.read_func_idx()?)),
            ELEMENT_SECTION => {
                log::debug!("Parsing element section...");
                self.read_section_inner(
                    len,
                    MAX_WASM_ELEMENT_SEGMENTS,
                    Self::read_element,
                    Section::Element,
                )
            }
            CODE_SECTION => {
                log::debug!("Parsing code section...");
                self.read_section_inner(
                    len,
                    MAX_WASM_FUNCTIONS,
                    Self::read_function_body,
                    Section::Code,
                )
            }
            DATA_SECTION => {
                log::debug!("Parsing data section...");
                self.read_section_inner(len, MAX_WASM_DATA_SEGMENTS, Self::read_data, Section::Data)
            }
            DATA_COUNT_SECTION => Ok(Section::DataCount(self.read_u32_leb128()?)),
            _ => Err(Error::UnknownSection(section_id)),
        }
    }

    fn read_section_inner<T>(
        &mut self,
        len: u32,
        limit: usize,
        ctor: fn(&mut BinaryReader<'a>) -> crate::Result<T>,
        variant: fn(VecReader<'a, T>) -> Section<'a>,
    ) -> crate::Result<Section<'a>> {
        let bytes = self.read_bytes(len as usize)?;

        let section_reader = VecReader::new(bytes, ctor, Some(limit))?;

        Ok(variant(section_reader))
    }

    pub fn read_instruction(&mut self) -> crate::Result<Instruction<'a>> {
        use crate::instructions::Instruction::*;
        let opcode = self.read_u8()?;

        Ok(match opcode {
            0x00 => Unreachable,
            0x01 => Nop,
            0x02 => Block {
                ty: self.read_block_type()?,
            },
            0x03 => Loop {
                ty: self.read_block_type()?,
            },
            0x04 => If {
                ty: self.read_block_type()?,
            },
            0x05 => Else,
            0x06 => Try {
                ty: self.read_block_type()?,
            },
            0x07 => Catch {
                tag: self.read_u32_leb128()?,
            },
            0x08 => Throw {
                tag: self.read_u32_leb128()?,
            },
            0x09 => Rethrow {
                relative_depth: self.read_u32_leb128()?,
            },
            0x0b => End,
            0x0c => Br {
                label: self.read_label_idx()?,
            },
            0x0d => BrIf {
                label: self.read_label_idx()?,
            },
            0x0e => BrTable {
                targets: self.read_br_table()?,
            },
            0x0f => Return,
            0x10 => Call {
                function: self.read_func_idx()?,
            },
            0x11 => CallIndirect {
                table: self.read_table_idx()?,
                ty: self.read_type_idx()?,
            },
            0x12 => ReturnCall {
                function: self.read_func_idx()?,
            },
            0x13 => ReturnCallIndirect {
                table: self.read_table_idx()?,
                ty: self.read_type_idx()?,
            },
            0x14 => CallRef {
                ty: self.read_type_idx()?,
            },
            0x15 => ReturnCallRef {
                ty: self.read_type_idx()?,
            },
            0x18 => Delegate {
                relative_depth: self.read_u32_leb128()?,
            },
            0x19 => CatchAll,
            0x1a => Drop,
            0x1b => Select,
            0x1c => {
                let len = self.read_u32_leb128()?;
                if len != 1 {
                    return Err(Error::InvalidTypedSelectArity(len));
                }

                TypedSelect {
                    ty: self.read_value_type()?,
                }
            }
            0x20 => LocalGet {
                local: self.read_local_idx()?,
            },
            0x21 => LocalSet {
                local: self.read_local_idx()?,
            },
            0x22 => LocalTee {
                local: self.read_local_idx()?,
            },
            0x23 => GlobalGet {
                global: self.read_global_idx()?,
            },
            0x24 => GlobalSet {
                global: self.read_global_idx()?,
            },
            0x25 => TableGet {
                table: self.read_table_idx()?,
            },
            0x26 => TableSet {
                table: self.read_table_idx()?,
            },

            0x28 => I32Load {
                memarg: self.read_memarg()?,
            },
            0x29 => I64Load {
                memarg: self.read_memarg()?,
            },
            0x2a => F32Load {
                memarg: self.read_memarg()?,
            },
            0x2b => F64Load {
                memarg: self.read_memarg()?,
            },
            0x2c => I32Load8S {
                memarg: self.read_memarg()?,
            },
            0x2d => I32Load8U {
                memarg: self.read_memarg()?,
            },
            0x2e => I32Load16S {
                memarg: self.read_memarg()?,
            },
            0x2f => I32Load16U {
                memarg: self.read_memarg()?,
            },
            0x30 => I64Load8S {
                memarg: self.read_memarg()?,
            },
            0x31 => I64Load8U {
                memarg: self.read_memarg()?,
            },
            0x32 => I64Load16S {
                memarg: self.read_memarg()?,
            },
            0x33 => I64Load16U {
                memarg: self.read_memarg()?,
            },
            0x34 => I64Load32S {
                memarg: self.read_memarg()?,
            },
            0x35 => I64Load32U {
                memarg: self.read_memarg()?,
            },
            0x36 => I32Store {
                memarg: self.read_memarg()?,
            },
            0x37 => I64Store {
                memarg: self.read_memarg()?,
            },
            0x38 => F32Store {
                memarg: self.read_memarg()?,
            },
            0x39 => F64Store {
                memarg: self.read_memarg()?,
            },
            0x3a => I32Store8 {
                memarg: self.read_memarg()?,
            },
            0x3b => I32Store16 {
                memarg: self.read_memarg()?,
            },
            0x3c => I64Store8 {
                memarg: self.read_memarg()?,
            },
            0x3d => I64Store16 {
                memarg: self.read_memarg()?,
            },
            0x3e => I64Store32 {
                memarg: self.read_memarg()?,
            },
            0x3f => MemorySize {
                mem: self.read_mem_idx()?,
            },
            0x40 => MemoryGrow {
                mem: self.read_mem_idx()?,
            },
            0x41 => I32Const {
                value: self.read_i32_leb128()?,
            },
            0x42 => I64Const {
                value: self.read_i64_leb128()?,
            },
            0x43 => F32Const {
                value: self.read_f32()?,
            },
            0x44 => F64Const {
                value: self.read_f64()?,
            },
            0x45 => I32Eqz,
            0x46 => I32Eq,
            0x47 => I32Ne,
            0x48 => I32LtS,
            0x49 => I32LtU,
            0x4a => I32GtS,
            0x4b => I32GtU,
            0x4c => I32LeS,
            0x4d => I32LeU,
            0x4e => I32GeS,
            0x4f => I32GeU,

            0x50 => I64Eqz,
            0x51 => I64Eq,
            0x52 => I64Ne,
            0x53 => I64LtS,
            0x54 => I64LtU,
            0x55 => I64GtS,
            0x56 => I64GtU,
            0x57 => I64LeS,
            0x58 => I64LeU,
            0x59 => I64GeS,
            0x5a => I64GeU,

            0x5b => F32Eq,
            0x5c => F32Ne,
            0x5d => F32Lt,
            0x5e => F32Gt,
            0x5f => F32Le,
            0x60 => F32Ge,

            0x61 => F64Eq,
            0x62 => F64Ne,
            0x63 => F64Lt,
            0x64 => F64Gt,
            0x65 => F64Le,
            0x66 => F64Ge,

            0x67 => I32Clz,
            0x68 => I32Ctz,
            0x69 => I32Popcnt,
            0x6a => I32Add,
            0x6b => I32Sub,
            0x6c => I32Mul,
            0x6d => I32DivS,
            0x6e => I32DivU,
            0x6f => I32RemS,
            0x70 => I32RemU,
            0x71 => I32And,
            0x72 => I32Or,
            0x73 => I32Xor,
            0x74 => I32Shl,
            0x75 => I32ShrS,
            0x76 => I32ShrU,
            0x77 => I32Rotl,
            0x78 => I32Rotr,

            0x79 => I64Clz,
            0x7a => I64Ctz,
            0x7b => I64Popcnt,
            0x7c => I64Add,
            0x7d => I64Sub,
            0x7e => I64Mul,
            0x7f => I64DivS,
            0x80 => I64DivU,
            0x81 => I64RemS,
            0x82 => I64RemU,
            0x83 => I64And,
            0x84 => I64Or,
            0x85 => I64Xor,
            0x86 => I64Shl,
            0x87 => I64ShrS,
            0x88 => I64ShrU,
            0x89 => I64Rotl,
            0x8a => I64Rotr,

            0x8b => F32Abs,
            0x8c => F32Neg,
            0x8d => F32Ceil,
            0x8e => F32Floor,
            0x8f => F32Trunc,
            0x90 => F32Nearest,
            0x91 => F32Sqrt,
            0x92 => F32Add,
            0x93 => F32Sub,
            0x94 => F32Mul,
            0x95 => F32Div,
            0x96 => F32Min,
            0x97 => F32Max,
            0x98 => F32Copysign,

            0x99 => F64Abs,
            0x9a => F64Neg,
            0x9b => F64Ceil,
            0x9c => F64Floor,
            0x9d => F64Trunc,
            0x9e => F64Nearest,
            0x9f => F64Sqrt,
            0xa0 => F64Add,
            0xa1 => F64Sub,
            0xa2 => F64Mul,
            0xa3 => F64Div,
            0xa4 => F64Min,
            0xa5 => F64Max,
            0xa6 => F64Copysign,
            0xa7 => I32WrapI64,
            0xa8 => I32TruncF32S,
            0xa9 => I32TruncF32U,
            0xaa => I32TruncF64S,
            0xab => I32TruncF64U,
            0xac => I64ExtendI32S,
            0xad => I64ExtendI32U,
            0xae => I64TruncF32S,
            0xaf => I64TruncF32U,
            0xb0 => I64TruncF64S,
            0xb1 => I64TruncF64U,
            0xb2 => F32ConvertI32S,
            0xb3 => F32ConvertI32U,
            0xb4 => F32ConvertI64S,
            0xb5 => F32ConvertI64U,
            0xb6 => F32DemoteF64,
            0xb7 => F64ConvertI32S,
            0xb8 => F64ConvertI32U,
            0xb9 => F64ConvertI64S,
            0xba => F64ConvertI64U,
            0xbb => F64PromoteF32,
            0xbc => I32ReinterpretF32,
            0xbd => I64ReinterpretF64,
            0xbe => F32ReinterpretI32,
            0xbf => F64ReinterpretI64,

            t => return Err(Error::UnknownInstruction(t)),
        })
    }
}
