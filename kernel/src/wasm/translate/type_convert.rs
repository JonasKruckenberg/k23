use crate::wasm::indices::{CanonicalizedTypeIndex, ModuleInternedTypeIndex, TypeIndex};
use crate::wasm::translate::types::{
    WasmArrayType, WasmCompositeType, WasmCompositeTypeInner, WasmFieldType, WasmHeapType,
    WasmHeapTypeInner, WasmRefType, WasmStorageType, WasmStructType, WasmValType,
};
use crate::wasm::translate::{ModuleTypes, TranslatedModule, WasmFuncType, WasmSubType};
use alloc::vec::Vec;
use wasmparser::UnpackedIndex;

/// A type that knows how to convert from `wasmparser` types to types in this crate.
pub struct WasmparserTypeConverter<'a> {
    types: &'a ModuleTypes,
    module: &'a TranslatedModule,
}

impl<'a> WasmparserTypeConverter<'a> {
    pub fn new(types: &'a ModuleTypes, module: &'a TranslatedModule) -> Self {
        Self { types, module }
    }

    pub fn convert_val_type(&self, ty: wasmparser::ValType) -> WasmValType {
        use wasmparser::ValType;
        match ty {
            ValType::I32 => WasmValType::I32,
            ValType::I64 => WasmValType::I64,
            ValType::F32 => WasmValType::F32,
            ValType::F64 => WasmValType::F64,
            ValType::V128 => WasmValType::V128,
            ValType::Ref(ty) => WasmValType::Ref(self.convert_ref_type(ty)),
        }
    }

    pub fn convert_ref_type(&self, ty: wasmparser::RefType) -> WasmRefType {
        WasmRefType {
            nullable: ty.is_nullable(),
            heap_type: self.convert_heap_type(ty.heap_type()),
        }
    }

    pub fn convert_heap_type(&self, ty: wasmparser::HeapType) -> WasmHeapType {
        match ty {
            wasmparser::HeapType::Concrete(index) => self.lookup_heap_type(index),
            wasmparser::HeapType::Abstract { shared, ty } => {
                use crate::wasm::translate::types::WasmHeapTypeInner::*;

                use wasmparser::AbstractHeapType;
                let ty = match ty {
                    AbstractHeapType::Func => Func,
                    AbstractHeapType::Extern => Extern,
                    AbstractHeapType::Any => Any,
                    AbstractHeapType::None => None,
                    AbstractHeapType::NoExtern => NoExtern,
                    AbstractHeapType::NoFunc => NoFunc,
                    AbstractHeapType::Eq => Eq,
                    AbstractHeapType::Struct => Struct,
                    AbstractHeapType::Array => Array,
                    AbstractHeapType::I31 => I31,
                    AbstractHeapType::Exn => Exn,
                    AbstractHeapType::NoExn => NoExn,
                    AbstractHeapType::Cont => Cont,
                    AbstractHeapType::NoCont => NoCont,
                };

                WasmHeapType { shared, inner: ty }
            }
        }
    }

    pub fn convert_sub_type(&self, ty: &wasmparser::SubType) -> WasmSubType {
        WasmSubType {
            is_final: ty.is_final,
            supertype: ty.supertype_idx.map(|index| {
                CanonicalizedTypeIndex::Module(self.lookup_type_index(index.unpack()))
            }),
            composite_type: self.convert_composite_type(&ty.composite_type),
        }
    }

    pub fn convert_composite_type(&self, ty: &wasmparser::CompositeType) -> WasmCompositeType {
        use wasmparser::CompositeInnerType;
        match &ty.inner {
            CompositeInnerType::Func(func) => {
                WasmCompositeType::new_func(ty.shared, self.convert_func_type(func))
            }
            CompositeInnerType::Array(array) => {
                WasmCompositeType::new_array(ty.shared, self.convert_array_type(*array))
            }
            CompositeInnerType::Struct(strct) => {
                WasmCompositeType::new_struct(ty.shared, self.convert_struct_type(strct))
            }
            CompositeInnerType::Cont(_) => todo!(),
        }
    }

    pub fn convert_func_type(&self, ty: &wasmparser::FuncType) -> WasmFuncType {
        let mut params = Vec::with_capacity(ty.params().len());
        let mut results = Vec::with_capacity(ty.results().len());

        for param in ty.params() {
            params.push(self.convert_val_type(*param));
        }

        for result in ty.results() {
            results.push(self.convert_val_type(*result));
        }

        WasmFuncType {
            params: params.into_boxed_slice(),
            results: results.into_boxed_slice(),
        }
    }

    pub fn convert_array_type(&self, ty: wasmparser::ArrayType) -> WasmArrayType {
        WasmArrayType(self.convert_field_type(ty.0))
    }

    pub fn convert_struct_type(&self, ty: &wasmparser::StructType) -> WasmStructType {
        let fields: Vec<_> = ty
            .fields
            .iter()
            .map(|ty| self.convert_field_type(*ty))
            .collect();
        WasmStructType {
            fields: fields.into_boxed_slice(),
        }
    }

    pub fn convert_field_type(&self, ty: wasmparser::FieldType) -> WasmFieldType {
        WasmFieldType {
            mutable: ty.mutable,
            element_type: self.convert_storage_type(ty.element_type),
        }
    }

    pub fn convert_storage_type(&self, ty: wasmparser::StorageType) -> WasmStorageType {
        use wasmparser::StorageType;
        match ty {
            StorageType::I8 => WasmStorageType::I8,
            StorageType::I16 => WasmStorageType::I16,
            StorageType::Val(ty) => WasmStorageType::Val(self.convert_val_type(ty)),
        }
    }

    fn lookup_type_index(&self, index: UnpackedIndex) -> ModuleInternedTypeIndex {
        match index {
            UnpackedIndex::Module(index) => {
                let module_index = TypeIndex::from_u32(index);
                self.module.types[module_index]
            }
            UnpackedIndex::Id(id) => self.types.seen_types[&id],
            UnpackedIndex::RecGroup(_) => unreachable!(),
        }
    }

    fn lookup_heap_type(&self, index: UnpackedIndex) -> WasmHeapType {
        match index {
            UnpackedIndex::Module(module_index) => {
                let module_index = TypeIndex::from_u32(module_index);
                let index = self.module.types[module_index];
                if let Some(ty) = self.types.get_wasm_type(index) {
                    match ty.composite_type.inner {
                        WasmCompositeTypeInner::Func(_) => WasmHeapType::new(
                            ty.composite_type.shared,
                            WasmHeapTypeInner::ConcreteFunc(CanonicalizedTypeIndex::Module(index)),
                        ),
                        WasmCompositeTypeInner::Array(_) => WasmHeapType::new(
                            ty.composite_type.shared,
                            WasmHeapTypeInner::ConcreteArray(CanonicalizedTypeIndex::Module(index)),
                        ),
                        WasmCompositeTypeInner::Struct(_) => WasmHeapType::new(
                            ty.composite_type.shared,
                            WasmHeapTypeInner::ConcreteStruct(CanonicalizedTypeIndex::Module(
                                index,
                            )),
                        ),
                    }
                } else {
                    todo!()
                }
            }
            UnpackedIndex::Id(id) => {
                let index = self.types.seen_types[&id];
                if let Some(ty) = self.types.get_wasm_type(index) {
                    match ty.composite_type.inner {
                        WasmCompositeTypeInner::Func(_) => WasmHeapType::new(
                            ty.composite_type.shared,
                            WasmHeapTypeInner::ConcreteFunc(CanonicalizedTypeIndex::Module(index)),
                        ),
                        WasmCompositeTypeInner::Array(_) => WasmHeapType::new(
                            ty.composite_type.shared,
                            WasmHeapTypeInner::ConcreteArray(CanonicalizedTypeIndex::Module(index)),
                        ),
                        WasmCompositeTypeInner::Struct(_) => WasmHeapType::new(
                            ty.composite_type.shared,
                            WasmHeapTypeInner::ConcreteStruct(CanonicalizedTypeIndex::Module(
                                index,
                            )),
                        ),
                    }
                } else {
                    todo!()
                }
            }
            UnpackedIndex::RecGroup(_) => unreachable!(),
        }
    }
}
