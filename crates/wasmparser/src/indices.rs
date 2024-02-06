use crate::BinaryReader;
use cranelift_entity::entity_impl;

pub trait FromBinaryReader<'a> {
    fn from_binary_reader(reader: &mut BinaryReader<'a>) -> Self;
}

macro_rules! impl_read {
    ($name:ident, $read_fn_name:ident) => {
        impl<'a> BinaryReader<'a> {
            pub fn $read_fn_name(&mut self) -> crate::Result<$name> {
                Ok($name(self.read_u32_leb128()?))
            }
        }

        impl<'a> TryFrom<&mut BinaryReader<'a>> for $name {
            type Error = crate::Error;
            fn try_from(reader: &mut BinaryReader<'a>) -> crate::Result<$name> {
                Ok($name(reader.read_u32_leb128()?))
            }
        }
    };
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeIdx(u32);
entity_impl!(TypeIdx, "wasm-type");
impl_read!(TypeIdx, read_type_idx);

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FuncIdx(u32);
entity_impl!(FuncIdx, "wasm-func");
impl_read!(FuncIdx, read_func_idx);

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TableIdx(u32);
entity_impl!(TableIdx, "wasm-table");
impl_read!(TableIdx, read_table_idx);

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemIdx(u32);
entity_impl!(MemIdx, "wasm-memory");
impl_read!(MemIdx, read_mem_idx);

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobalIdx(u32);
entity_impl!(GlobalIdx, "wasm-global");
impl_read!(GlobalIdx, read_global_idx);

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElemIdx(u32);
entity_impl!(ElemIdx, "wasm-element");
impl_read!(ElemIdx, read_elem_idx);

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DataIdx(u32);
entity_impl!(DataIdx, "wasm-data");
impl_read!(DataIdx, read_data_idx);

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalIdx(u32);
entity_impl!(LocalIdx, "wasm-local");
impl_read!(LocalIdx, read_local_idx);

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LabelIdx(u32);
entity_impl!(LabelIdx, "wasm-label");
impl_read!(LabelIdx, read_label_idx);
