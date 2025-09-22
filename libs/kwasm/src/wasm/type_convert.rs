use alloc::vec::Vec;

use cranelift_entity::EntityRef;
use wasmparser::UnpackedIndex;

use crate::indices::{CanonicalizedTypeIndex, ModuleInternedTypeIndex, TypeIndex};
use crate::wasm::{
    ModuleTypes, TranslatedModule, WasmArrayType, WasmCompositeType, WasmCompositeTypeInner,
    WasmFieldType, WasmFuncType, WasmHeapType, WasmHeapTypeInner, WasmRefType, WasmStorageType,
    WasmStructType, WasmSubType, WasmValType,
};

/// A type that knows how to convert from `wasmparser` types to types in this crate.
pub struct WasmparserTypeConverter<'a> {
    types: &'a ModuleTypes,
    module: &'a TranslatedModule,
    rec_group_context: Option<(
        wasmparser::types::TypesRef<'a>,
        wasmparser::types::RecGroupId,
    )>,
}

impl<'a> WasmparserTypeConverter<'a> {
    pub fn new(types: &'a ModuleTypes, module: &'a TranslatedModule) -> Self {
        Self {
            types,
            module,
            rec_group_context: None,
        }
    }

    /// Configure this converter to be within the context of defining the
    /// current rec group.
    pub fn with_rec_group(
        &mut self,
        wasmparser_types: wasmparser::types::TypesRef<'a>,
        rec_group: wasmparser::types::RecGroupId,
    ) -> &Self {
        self.rec_group_context = Some((wasmparser_types, rec_group));
        self
    }

    pub fn convert_val_type(&self, ty: wasmparser::ValType) -> WasmValType {
        match ty {
            wasmparser::ValType::I32 => WasmValType::I32,
            wasmparser::ValType::I64 => WasmValType::I64,
            wasmparser::ValType::F32 => WasmValType::F32,
            wasmparser::ValType::F64 => WasmValType::F64,
            wasmparser::ValType::V128 => WasmValType::V128,
            wasmparser::ValType::Ref(ty) => WasmValType::Ref(self.convert_ref_type(ty)),
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
                use wasmparser::AbstractHeapType;

                use crate::wasm::types::WasmHeapTypeInner::*;
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
        match ty {
            wasmparser::StorageType::I8 => WasmStorageType::I8,
            wasmparser::StorageType::I16 => WasmStorageType::I16,
            wasmparser::StorageType::Val(ty) => WasmStorageType::Val(self.convert_val_type(ty)),
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
            UnpackedIndex::Id(id) => {
                let interned = self.types.seen_types[&id];
                let index = CanonicalizedTypeIndex::Module(interned);

                // If this is a forward reference to a type in this type's rec
                // group that we haven't converted yet, then we won't have an
                // entry in `wasm_types` yet. In this case, fallback to a
                // different means of determining whether this is a concrete
                // array vs struct vs func reference. In this case, we can use
                // the validator's type context.
                if let Some(ty) = self.types.get_wasm_type(interned) {
                    let inner = match &ty.composite_type.inner {
                        WasmCompositeTypeInner::Array(_) => WasmHeapTypeInner::ConcreteArray(index),
                        WasmCompositeTypeInner::Func(_) => WasmHeapTypeInner::ConcreteFunc(index),
                        WasmCompositeTypeInner::Struct(_) => {
                            WasmHeapTypeInner::ConcreteStruct(index)
                        }
                    };

                    WasmHeapType::new(ty.composite_type.shared, inner)
                } else if let Some((wasmparser_types, _)) = self.rec_group_context.as_ref() {
                    let wasmparser_ty = &wasmparser_types[id].composite_type;
                    let inner = match &wasmparser_ty.inner {
                        wasmparser::CompositeInnerType::Array(_) => {
                            WasmHeapTypeInner::ConcreteArray(index)
                        }
                        wasmparser::CompositeInnerType::Func(_) => {
                            WasmHeapTypeInner::ConcreteFunc(index)
                        }
                        wasmparser::CompositeInnerType::Struct(_) => {
                            WasmHeapTypeInner::ConcreteStruct(index)
                        }
                        wasmparser::CompositeInnerType::Cont(_) => {
                            panic!("unimplemented continuation types")
                        }
                    };

                    WasmHeapType::new(wasmparser_ty.shared, inner)
                } else {
                    panic!("forward reference to type outside of rec group?")
                }
            }

            UnpackedIndex::Module(module_index) => {
                let module_index = TypeIndex::from_u32(module_index);
                let interned = self.module.types[module_index];
                let index = CanonicalizedTypeIndex::Module(interned);

                // See comment above about `wasm_types` maybe not having the
                // converted sub-type yet. However, in this case we don't have a
                // `wasmparser::types::CoreTypeId` on hand, so we have to
                // indirectly get one by looking it up inside the current rec
                // group.
                if let Some(ty) = self.types.get_wasm_type(interned) {
                    let inner = match &ty.composite_type.inner {
                        WasmCompositeTypeInner::Array(_) => WasmHeapTypeInner::ConcreteArray(index),
                        WasmCompositeTypeInner::Func(_) => WasmHeapTypeInner::ConcreteFunc(index),
                        WasmCompositeTypeInner::Struct(_) => {
                            WasmHeapTypeInner::ConcreteStruct(index)
                        }
                    };

                    WasmHeapType::new(ty.composite_type.shared, inner)
                } else if let Some((parser_types, rec_group)) = self.rec_group_context.as_ref() {
                    let rec_group_index = interned.index() - self.types.len_types();
                    let id = parser_types
                        .rec_group_elements(*rec_group)
                        .nth(rec_group_index)
                        .unwrap();
                    let wasmparser_ty = &parser_types[id].composite_type;

                    let inner = match &wasmparser_ty.inner {
                        wasmparser::CompositeInnerType::Array(_) => {
                            WasmHeapTypeInner::ConcreteArray(index)
                        }
                        wasmparser::CompositeInnerType::Func(_) => {
                            WasmHeapTypeInner::ConcreteFunc(index)
                        }
                        wasmparser::CompositeInnerType::Struct(_) => {
                            WasmHeapTypeInner::ConcreteStruct(index)
                        }
                        wasmparser::CompositeInnerType::Cont(_) => {
                            panic!("unimplemented continuation types")
                        }
                    };

                    WasmHeapType::new(wasmparser_ty.shared, inner)
                } else {
                    panic!("forward reference to type outside of rec group?")
                }
            }

            UnpackedIndex::RecGroup(_) => unreachable!(),
        }
    }
}
