use crate::types::ValueType;
use crate::vec_reader::{VecIter, VecReader};
use crate::{LabelIdx, MemIdx, TypeIdx};
use core::fmt;
use core::fmt::Formatter;

#[derive(Debug, Clone, Copy)]
pub enum BlockType {
    Empty,
    Type(ValueType),
    FunctionType(TypeIdx),
}

pub struct BrTable<'a> {
    pub(crate) labels: VecReader<'a, LabelIdx>,
    pub(crate) default: LabelIdx,
}

#[derive(Debug)]
pub struct MemArg {
    pub memory: MemIdx,
    pub align: u8,
    /// memory64 proposal allows for 64-bit offsets
    pub offset: u64,
}

#[derive(Debug, Copy, Clone)]
pub struct Ieee32(pub(crate) u32);

impl Ieee32 {
    pub const ZERO: Self = Self(0);

    pub fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub fn bits(self) -> u32 {
        self.0
    }

    pub fn as_f32(&self) -> f32 {
        self.0 as f32
    }
}

#[derive(Debug, Copy, Clone)]
pub struct Ieee64(pub(crate) u64);

impl Ieee64 {
    pub const ZERO: Self = Self(0);

    pub fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    pub fn bits(self) -> u64 {
        self.0
    }

    pub fn as_f64(&self) -> f64 {
        self.0 as f64
    }
}

#[derive(Debug)]
pub struct V128([u8; 16]);

impl<'a> BrTable<'a> {
    pub fn labels(&self) -> VecIter<'a, LabelIdx> {
        self.labels.iter()
    }
}

impl<'a> fmt::Debug for BrTable<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrTable")
            .field("labels", &self.labels)
            .field("default", &self.default)
            .finish()
    }
}

macro_rules! for_each_operator {
    ($mac:ident) => {
        $mac! {
            @mvp Unreachable
            @mvp Nop
            @mvp Block { ty: $crate::BlockType }
            @mvp Loop { ty: $crate::BlockType }
            @mvp If { ty: $crate::BlockType }
            @mvp Else
            @exceptions Try { ty: $crate::BlockType }
            @exceptions Catch { tag: u32 }
            @exceptions Throw { tag: u32 }
            @exceptions Rethrow { relative_depth: u32 }
            @mvp End
            @mvp Br { label: $crate::LabelIdx }
            @mvp BrIf { label: $crate::LabelIdx }
            @mvp BrTable { targets: $crate::BrTable<'a> }
            @mvp Return
            @mvp Call { function: $crate::FuncIdx }
            @mvp CallIndirect { ty: $crate::TypeIdx, table: $crate::TableIdx }
            @tail_call ReturnCall { function: $crate::FuncIdx }
            @tail_call ReturnCallIndirect { ty: $crate::TypeIdx, table: $crate::TableIdx }
            @exceptions Delegate { relative_depth: u32 }
            @exceptions CatchAll
            @mvp Drop
            @mvp Select
            @reference_types TypedSelect { ty: $crate::ValueType }
            @mvp LocalGet { local: $crate::LocalIdx }
            @mvp LocalSet { local: $crate::LocalIdx }
            @mvp LocalTee { local: $crate::LocalIdx }
            @mvp GlobalGet { global: $crate::GlobalIdx }
            @mvp GlobalSet { global: $crate::GlobalIdx }
            @mvp I32Load { memarg: $crate::MemArg }
            @mvp I64Load { memarg: $crate::MemArg }
            @mvp F32Load { memarg: $crate::MemArg }
            @mvp F64Load { memarg: $crate::MemArg }
            @mvp I32Load8S { memarg: $crate::MemArg }
            @mvp I32Load8U { memarg: $crate::MemArg }
            @mvp I32Load16S { memarg: $crate::MemArg }
            @mvp I32Load16U { memarg: $crate::MemArg }
            @mvp I64Load8S { memarg: $crate::MemArg }
            @mvp I64Load8U { memarg: $crate::MemArg }
            @mvp I64Load16S { memarg: $crate::MemArg }
            @mvp I64Load16U { memarg: $crate::MemArg }
            @mvp I64Load32S { memarg: $crate::MemArg }
            @mvp I64Load32U { memarg: $crate::MemArg }
            @mvp I32Store { memarg: $crate::MemArg }
            @mvp I64Store { memarg: $crate::MemArg }
            @mvp F32Store { memarg: $crate::MemArg }
            @mvp F64Store { memarg: $crate::MemArg }
            @mvp I32Store8 { memarg: $crate::MemArg }
            @mvp I32Store16 { memarg: $crate::MemArg }
            @mvp I64Store8 { memarg: $crate::MemArg }
            @mvp I64Store16 { memarg: $crate::MemArg }
            @mvp I64Store32 { memarg: $crate::MemArg }
            @mvp MemorySize { mem: $crate::MemIdx }
            @mvp MemoryGrow { mem: $crate::MemIdx }
            @mvp I32Const { value: i32 }
            @mvp I64Const { value: i64 }
            @mvp F32Const { value: $crate::Ieee32 }
            @mvp F64Const { value: $crate::Ieee64 }
            @reference_types RefNull { ty: $crate::ReferenceType }
            @reference_types RefIsNull
            @reference_types RefFunc { function: $crate::FuncIdx }

            @mvp I32Eqz
            @mvp I32Eq
            @mvp I32Ne
            @mvp I32LtS
            @mvp I32LtU
            @mvp I32GtS
            @mvp I32GtU
            @mvp I32LeS
            @mvp I32LeU
            @mvp I32GeS
            @mvp I32GeU
            @mvp I64Eqz
            @mvp I64Eq
            @mvp I64Ne
            @mvp I64LtS
            @mvp I64LtU
            @mvp I64GtS
            @mvp I64GtU
            @mvp I64LeS
            @mvp I64LeU
            @mvp I64GeS
            @mvp I64GeU

            @mvp F32Eq
            @mvp F32Ne
            @mvp F32Lt
            @mvp F32Gt
            @mvp F32Le
            @mvp F32Ge
            @mvp F64Eq
            @mvp F64Ne
            @mvp F64Lt
            @mvp F64Gt
            @mvp F64Le
            @mvp F64Ge

            @mvp I32Clz
            @mvp I32Ctz
            @mvp I32Popcnt
            @mvp I32Add
            @mvp I32Sub
            @mvp I32Mul
            @mvp I32DivS
            @mvp I32DivU
            @mvp I32RemS
            @mvp I32RemU
            @mvp I32And
            @mvp I32Or
            @mvp I32Xor
            @mvp I32Shl
            @mvp I32ShrS
            @mvp I32ShrU
            @mvp I32Rotl
            @mvp I32Rotr
            @mvp I64Clz
            @mvp I64Ctz
            @mvp I64Popcnt
            @mvp I64Add
            @mvp I64Sub
            @mvp I64Mul
            @mvp I64DivS
            @mvp I64DivU
            @mvp I64RemS
            @mvp I64RemU
            @mvp I64And
            @mvp I64Or
            @mvp I64Xor
            @mvp I64Shl
            @mvp I64ShrS
            @mvp I64ShrU
            @mvp I64Rotl
            @mvp I64Rotr

            @mvp F32Abs
            @mvp F32Neg
            @mvp F32Ceil
            @mvp F32Floor
            @mvp F32Trunc
            @mvp F32Nearest
            @mvp F32Sqrt
            @mvp F32Add
            @mvp F32Sub
            @mvp F32Mul
            @mvp F32Div
            @mvp F32Min
            @mvp F32Max
            @mvp F32Copysign
            @mvp F64Abs
            @mvp F64Neg
            @mvp F64Ceil
            @mvp F64Floor
            @mvp F64Trunc
            @mvp F64Nearest
            @mvp F64Sqrt
            @mvp F64Add
            @mvp F64Sub
            @mvp F64Mul
            @mvp F64Div
            @mvp F64Min
            @mvp F64Max
            @mvp F64Copysign

            @mvp I32WrapI64
            @mvp I32TruncF32S
            @mvp I32TruncF32U
            @mvp I32TruncF64S
            @mvp I32TruncF64U
            @mvp I64ExtendI32S
            @mvp I64ExtendI32U
            @mvp I64TruncF32S
            @mvp I64TruncF32U
            @mvp I64TruncF64S
            @mvp I64TruncF64U
            @mvp F32ConvertI32S
            @mvp F32ConvertI32U
            @mvp F32ConvertI64S
            @mvp F32ConvertI64U
            @mvp F32DemoteF64
            @mvp F64ConvertI32S
            @mvp F64ConvertI32U
            @mvp F64ConvertI64S
            @mvp F64ConvertI64U
            @mvp F64PromoteF32
            @mvp I32ReinterpretF32
            @mvp I64ReinterpretF64
            @mvp F32ReinterpretI32
            @mvp F64ReinterpretI64
            @sign_extension I32Extend8S
            @sign_extension I32Extend16S
            @sign_extension I64Extend8S
            @sign_extension I64Extend16S
            @sign_extension I64Extend32S

            // Garbage Collection Proposal
            @gc RefI31
            @gc I31GetS
            @gc I31GetU

            // Non-trapping Float-to-int Conversions Proposal
            @nontrapping_float_to_int I32TruncSatF32S
            @nontrapping_float_to_int I32TruncSatF32U
            @nontrapping_float_to_int I32TruncSatF64S
            @nontrapping_float_to_int I32TruncSatF64U
            @nontrapping_float_to_int I64TruncSatF32S
            @nontrapping_float_to_int I64TruncSatF32U
            @nontrapping_float_to_int I64TruncSatF64S
            @nontrapping_float_to_int I64TruncSatF64U

            // Bulk Memory Operations Proposal
            @bulk_memory MemoryInit { data: $crate::DataIdx, mem: $crate::MemIdx }
            @bulk_memory DataDrop { data: $crate::DataIdx }
            @bulk_memory MemoryCopy { dst_mem: $crate::MemIdx, src_mem: $crate::MemIdx }
            @bulk_memory MemoryFill { mem: $crate::MemIdx }
            @bulk_memory TableInit { element: $crate::ElemIdx , table: $crate::TableIdx }
            @bulk_memory ElemDrop { element: $crate::ElemIdx }
            @bulk_memory TableCopy { dst_table: $crate::TableIdx, src_table: $crate::TableIdx }

            // Reference Types Proposal
            @reference_types TableFill { table: $crate::TableIdx }
            @reference_types TableGet { table: $crate::TableIdx }
            @reference_types TableSet { table: $crate::TableIdx }
            @reference_types TableGrow { table: $crate::TableIdx }
            @reference_types TableSize { table: $crate::TableIdx }

            // Memory Control Proposal
            @memory_control MemoryDiscard { mem: $crate::MemIdx }

            // Threads Proposal
            @threads MemoryAtomicNotify { memarg: $crate::MemArg }
            @threads MemoryAtomicWait32 { memarg: $crate::MemArg }
            @threads MemoryAtomicWait64 { memarg: $crate::MemArg }
            @threads AtomicFence
            @threads I32AtomicLoad { memarg: $crate::MemArg }
            @threads I64AtomicLoad { memarg: $crate::MemArg }
            @threads I32AtomicLoad8U { memarg: $crate::MemArg }
            @threads I32AtomicLoad16U { memarg: $crate::MemArg }
            @threads I64AtomicLoad8U { memarg: $crate::MemArg }
            @threads I64AtomicLoad16U { memarg: $crate::MemArg }
            @threads I64AtomicLoad32U { memarg: $crate::MemArg }
            @threads I32AtomicStore { memarg: $crate::MemArg }
            @threads I64AtomicStore { memarg: $crate::MemArg }
            @threads I32AtomicStore8 { memarg: $crate::MemArg }
            @threads I32AtomicStore16 { memarg: $crate::MemArg }
            @threads I64AtomicStore8 { memarg: $crate::MemArg }
            @threads I64AtomicStore16 { memarg: $crate::MemArg }
            @threads I64AtomicStore32 { memarg: $crate::MemArg }
            @threads I32AtomicRmwAdd { memarg: $crate::MemArg }
            @threads I64AtomicRmwAdd { memarg: $crate::MemArg }
            @threads I32AtomicRmw8AddU { memarg: $crate::MemArg }
            @threads I32AtomicRmw16AddU { memarg: $crate::MemArg }
            @threads I64AtomicRmw8AddU { memarg: $crate::MemArg }
            @threads I64AtomicRmw16AddU { memarg: $crate::MemArg }
            @threads I64AtomicRmw32AddU { memarg: $crate::MemArg }
            @threads I32AtomicRmwSub { memarg: $crate::MemArg }
            @threads I64AtomicRmwSub { memarg: $crate::MemArg }
            @threads I32AtomicRmw8SubU { memarg: $crate::MemArg }
            @threads I32AtomicRmw16SubU { memarg: $crate::MemArg }
            @threads I64AtomicRmw8SubU { memarg: $crate::MemArg }
            @threads I64AtomicRmw16SubU { memarg: $crate::MemArg }
            @threads I64AtomicRmw32SubU { memarg: $crate::MemArg }
            @threads I32AtomicRmwAnd { memarg: $crate::MemArg }
            @threads I64AtomicRmwAnd { memarg: $crate::MemArg }
            @threads I32AtomicRmw8AndU { memarg: $crate::MemArg }
            @threads I32AtomicRmw16AndU { memarg: $crate::MemArg }
            @threads I64AtomicRmw8AndU { memarg: $crate::MemArg }
            @threads I64AtomicRmw16AndU { memarg: $crate::MemArg }
            @threads I64AtomicRmw32AndU { memarg: $crate::MemArg }
            @threads I32AtomicRmwOr { memarg: $crate::MemArg }
            @threads I64AtomicRmwOr { memarg: $crate::MemArg }
            @threads I32AtomicRmw8OrU { memarg: $crate::MemArg }
            @threads I32AtomicRmw16OrU { memarg: $crate::MemArg }
            @threads I64AtomicRmw8OrU { memarg: $crate::MemArg }
            @threads I64AtomicRmw16OrU { memarg: $crate::MemArg }
            @threads I64AtomicRmw32OrU { memarg: $crate::MemArg }
            @threads I32AtomicRmwXor { memarg: $crate::MemArg }
            @threads I64AtomicRmwXor { memarg: $crate::MemArg }
            @threads I32AtomicRmw8XorU { memarg: $crate::MemArg }
            @threads I32AtomicRmw16XorU { memarg: $crate::MemArg }
            @threads I64AtomicRmw8XorU { memarg: $crate::MemArg }
            @threads I64AtomicRmw16XorU { memarg: $crate::MemArg }
            @threads I64AtomicRmw32XorU { memarg: $crate::MemArg }
            @threads I32AtomicRmwXchg { memarg: $crate::MemArg }
            @threads I64AtomicRmwXchg { memarg: $crate::MemArg }
            @threads I32AtomicRmw8XchgU { memarg: $crate::MemArg }
            @threads I32AtomicRmw16XchgU { memarg: $crate::MemArg }
            @threads I64AtomicRmw8XchgU { memarg: $crate::MemArg }
            @threads I64AtomicRmw16XchgU { memarg: $crate::MemArg }
            @threads I64AtomicRmw32XchgU { memarg: $crate::MemArg }
            @threads I32AtomicRmwCmpxchg { memarg: $crate::MemArg }
            @threads I64AtomicRmwCmpxchg { memarg: $crate::MemArg }
            @threads I32AtomicRmw8CmpxchgU { memarg: $crate::MemArg }
            @threads I32AtomicRmw16CmpxchgU { memarg: $crate::MemArg }
            @threads I64AtomicRmw8CmpxchgU { memarg: $crate::MemArg }
            @threads I64AtomicRmw16CmpxchgU { memarg: $crate::MemArg }
            @threads I64AtomicRmw32CmpxchgU { memarg: $crate::MemArg }

            // 128-bit SIMP Proposal
            @simd V128Load { memarg: $crate::MemArg }
            @simd V128Load8x8S { memarg: $crate::MemArg }
            @simd V128Load8x8U { memarg: $crate::MemArg }
            @simd V128Load16x4S { memarg: $crate::MemArg }
            @simd V128Load16x4U { memarg: $crate::MemArg }
            @simd V128Load32x2S { memarg: $crate::MemArg }
            @simd V128Load32x2U { memarg: $crate::MemArg }
            @simd V128Load8Splat { memarg: $crate::MemArg }
            @simd V128Load16Splat { memarg: $crate::MemArg }
            @simd V128Load32Splat { memarg: $crate::MemArg }
            @simd V128Load64Splat { memarg: $crate::MemArg }
            @simd V128Load32Zero { memarg: $crate::MemArg }
            @simd V128Load64Zero { memarg: $crate::MemArg }
            @simd V128Store { memarg: $crate::MemArg }
            @simd V128Load8Lane { memarg: $crate::MemArg }
            @simd V128Load16Lane { memarg: $crate::MemArg }
            @simd V128Load32Lane { memarg: $crate::MemArg }
            @simd V128Load64Lane { memarg: $crate::MemArg }
            @simd V128Store8Lane { memarg: $crate::MemArg }
            @simd V128Store16Lane { memarg: $crate::MemArg }
            @simd V128Store32Lane { memarg: $crate::MemArg }
            @simd V128Store64Lane { memarg: $crate::MemArg }

            @simd V128Const { value: $crate::V128 }

            @simd I8x16Shuffle { lanes: [u8; 16] }

            @simd I8x16ExtractLaneS { lane: u8 }
            @simd I8x16ExtractLaneU { lane: u8 }
            @simd I8x16ReplaceLane { lane: u8 }
            @simd I16x8ExtractLaneS { lane: u8 }
            @simd I16x8ExtractLaneU { lane: u8 }
            @simd I16x8ReplaceLane { lane: u8 }
            @simd I32x4ExtractLane { lane: u8 }
            @simd I32x4ReplaceLane { lane: u8 }
            @simd I64x2ExtractLane { lane: u8 }
            @simd I64x2ReplaceLane { lane: u8 }
            @simd F32x4ExtractLane { lane: u8 }
            @simd F32x4ReplaceLane { lane: u8 }
            @simd F64x2ExtractLane { lane: u8 }
            @simd F64x2ReplaceLane { lane: u8 }

            @simd I8x16Swizzle
            @simd I8x16Splat
            @simd I16x8Splat
            @simd I32x4Splat
            @simd I64x2Splat
            @simd F32x4Splat
            @simd F64x2Splat

            @simd I8x16Eq
            @simd I8x16Ne
            @simd I8x16LtS
            @simd I8x16LtU
            @simd I8x16GtS
            @simd I8x16GtU
            @simd I8x16LeS
            @simd I8x16LeU
            @simd I8x16GeS
            @simd I8x16GeU

            @simd I16x8Eq
            @simd I16x8Ne
            @simd I16x8LtS
            @simd I16x8LtU
            @simd I16x8GtS
            @simd I16x8GtU
            @simd I16x8LeS
            @simd I16x8LeU
            @simd I16x8GeS
            @simd I16x8GeU

            @simd I32x4Eq
            @simd I32x4Ne
            @simd I32x4LtS
            @simd I32x4LtU
            @simd I32x4GtS
            @simd I32x4GtU
            @simd I32x4LeS
            @simd I32x4LeU
            @simd I32x4GeS
            @simd I32x4GeU

            @simd I64x2Eq
            @simd I64x2Ne
            @simd I64x2LtS
            @simd I64x2GtS
            @simd I64x2LeS
            @simd I64x2GeS

            @simd F32x4Eq
            @simd F32x4Ne
            @simd F32x4Lt
            @simd F32x4Gt
            @simd F32x4Le
            @simd F32x4Ge

            @simd F64x2Eq
            @simd F64x2Ne
            @simd F64x2Lt
            @simd F64x2Gt
            @simd F64x2Le
            @simd F64x2Ge

            @simd V128Not
            @simd V128And
            @simd V128AndNot
            @simd V128Or
            @simd V128Xor
            @simd V128Bitselect
            @simd AnyTrue

            @simd I8x16Abs
            @simd I8x16Neg
            @simd I8x16Popcnt
            @simd I8x16AllTrue
            @simd I8x16Bitmask
            @simd I8x16NarrowI16x8S
            @simd I8x16NarrowI16x8U
            @simd I8x16Shl
            @simd I8x16ShrS
            @simd I8x16ShrU
            @simd I8x16Add
            @simd I8x16AddSatS
            @simd I8x16AddSatU
            @simd I8x16Sub
            @simd I8x16SubSatS
            @simd I8x16SubSatU
            @simd I8x16MinS
            @simd I8x16MinU
            @simd I8x16MaxS
            @simd I8x16MaxU
            @simd I8x16AvgrU

            @simd I16x8ExtaddPairwiseI8x16S
            @simd I16x8ExtaddPairwiseI8x16U
            @simd I16x8Abs
            @simd I16x8Neg
            @simd I16x8Q15MulrSatS
            @simd I16x8AllTrue
            @simd I16x8Bitmask
            @simd I16x8NarrowI32x4S
            @simd I16x8NarrowI32x4U
            @simd I16x8ExtendLowI8x16S
            @simd I16x8ExtendHighI8x16S
            @simd I16x8ExtendLowI8x16U
            @simd I16x8ExtendHighI8x16U
            @simd I16x8Shl
            @simd I16x8ShrS
            @simd I16x8ShrU
            @simd I16x8Add
            @simd I16x8AddSatS
            @simd I16x8AddSatU
            @simd I16x8Sub
            @simd I16x8SubSatS
            @simd I16x8SubSatU
            @simd I16x8Mul
            @simd I16x8MinS
            @simd I16x8MinU
            @simd I16x8MaxS
            @simd I16x8MaxU
            @simd I16x8AvgrU
            @simd I16x8ExtmulLowI8x16S
            @simd I16x8ExtmulHighI8x16S
            @simd I16x8ExtmulLowI8x16U
            @simd I16x8ExtmulHighI8x16U

            @simd I32x4ExtaddPairwiseI16x8S
            @simd I32x4ExtaddPairwiseI16x8U
            @simd I32x4Abs
            @simd I32x4Neg
            @simd I32x4Q15MulrSatS
            @simd I32x4AllTrue
            @simd I32x4Bitmask
            @simd I32x4NarrowI32x4S
            @simd I32x4NarrowI32x4U
            @simd I32x4ExtendLowI16x8S
            @simd I32x4ExtendHighI16x8S
            @simd I32x4ExtendLowI16x8U
            @simd I32x4ExtendHighI16x8U
            @simd I32x4Shl
            @simd I32x4ShrS
            @simd I32x4ShrU
            @simd I32x4Add
            @simd I32x4Sub
            @simd I32x4Mul
            @simd I32x4MinS
            @simd I32x4MinU
            @simd I32x4MaxS
            @simd I32x4MaxU
            @simd I32x4DotI32x4S
            @simd I32x4ExtmulLowI16x8S
            @simd I32x4ExtmulHighI16x8S
            @simd I32x4ExtmulLowI16x8U
            @simd I32x4ExtmulHighI16x8U

            @simd I64x2Abs
            @simd I64x2Neg
            @simd I64x2AllTrue
            @simd I64x2Bitmask
            @simd I64x2ExtendLowI32x4S
            @simd I64x2ExtendHighI32x4S
            @simd I64x2ExtendLowI32x4U
            @simd I64x2ExtendHighI32x4U
            @simd I64x2Shl
            @simd I64x2ShrS
            @simd I64x2ShrU
            @simd I64x2Add
            @simd I64x2Sub
            @simd I64x2Mul
            @simd I64x2ExtmulLowI32x4S
            @simd I64x2ExtmulHighI32x4S
            @simd I64x2ExtmulLowI32x4U
            @simd I64x2ExtmulHighI32x4U

            @simd F32x4Ceil
            @simd F32x4Floor
            @simd F32x4Trunc
            @simd F32x4Nearest
            @simd F32x4Abs
            @simd F32x4Neg
            @simd F32x4Sqrt
            @simd F32x4Add
            @simd F32x4Sub
            @simd F32x4Mul
            @simd F32x4Div
            @simd F32x4Min
            @simd F32x4Max
            @simd F32x4Pmin
            @simd F32x4Pmax

            @simd F64x4Ceil
            @simd F64x4Floor
            @simd F64x4Trunc
            @simd F64x4Nearest
            @simd F64x4Abs
            @simd F64x4Neg
            @simd F64x4Sqrt
            @simd F64x4Add
            @simd F64x4Sub
            @simd F64x4Mul
            @simd F64x4Div
            @simd F64x4Min
            @simd F64x4Max
            @simd F64x4Pmin
            @simd F64x4Pmax

            @simd I32x4TruncSatF32x4S
            @simd I32x4TruncSatF32x4U
            @simd F32x4ConvertI32x4S
            @simd F32x4ConvertI32x4U
            @simd I32x4TruncSatF64x2SZero
            @simd I32x4TruncSatF64x2UZero
            @simd F64x2ConvertLowI32x4S
            @simd F64x2ConvertLowI32x4U
            @simd F32x4DemoteF64x2Zero
            @simd F64x2PromoteLowF32x4

            // Relaxed SIMD Proposal
            @relaxed_simd I8x16RelaxedSwizzle
            @relaxed_simd I32x4RelaxedTruncF32x4S
            @relaxed_simd I32x4RelaxedTruncF32x4U
            @relaxed_simd I32x4RelaxedTruncF64x2SZero
            @relaxed_simd I32x4RelaxedTruncF64x2UZero
            @relaxed_simd F32x4RelaxedMadd
            @relaxed_simd F32x4RelaxedNmadd
            @relaxed_simd F64x2RelaxedMadd
            @relaxed_simd F64x2RelaxedNmadd
            @relaxed_simd I8x16RelaxedLaneselect
            @relaxed_simd I16x8RelaxedLaneselect
            @relaxed_simd I32x4RelaxedLaneselect
            @relaxed_simd I64x2RelaxedLaneselect
            @relaxed_simd F32x4RelaxedMin
            @relaxed_simd F32x4RelaxedMax
            @relaxed_simd F64x2RelaxedMin
            @relaxed_simd F64x2RelaxedMax
            @relaxed_simd I16x8RelaxedQ15mulrS
            @relaxed_simd I16x8RelaxedDotI8x16I7x16S
            @relaxed_simd I32x4RelaxedDotI8x16I7x16AddS

            // Typed Function References Proposal
            @function_references CallRef { ty: $crate::TypeIdx }
            @function_references ReturnCallRef { ty: $crate::TypeIdx }
            @function_references RefAsNonNull
            @function_references BrOnNull { relative_depth: u32 }
            @function_references BrOnNonNull { relative_depth: u32 }
        }
    };
}

macro_rules! define_enum {
    ($(@$proposal:ident $instr:ident $({ $($payload:tt)* })?)*) => {
        #[derive(Debug)]
        pub enum Instruction<'a> {
            $(
                $instr $({ $($payload)* })?,
            )*
        }
    };
}

for_each_operator!(define_enum);
