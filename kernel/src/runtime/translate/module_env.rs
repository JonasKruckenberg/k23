use super::{FunctionType, Import, MemoryPlan, TablePlan, TranslatedModule};
use crate::runtime::FuncRefIndex;
use cranelift_codegen::entity::PrimaryMap;
use cranelift_codegen::packed_option::ReservedValue;
use cranelift_wasm::wasmparser::{
    Encoding, ExternalKind, FuncToValidate, FunctionBody, Parser, Payload, TypeRef, UnpackedIndex,
    Validator, ValidatorResources, WasmFeatures,
};
use cranelift_wasm::{
    ConstExpr, DefinedFuncIndex, EntityIndex, FuncIndex, GlobalIndex, MemoryIndex,
    ModuleInternedTypeIndex, TableIndex, TypeConvert, TypeIndex, WasmCompositeType, WasmHeapType,
    WasmResult, WasmSubType,
};

pub struct ModuleEnvironment<'a, 'wasm> {
    result: ModuleTranslation<'wasm>,
    validator: &'a mut Validator,
}

#[derive(Default)]
pub struct ModuleTranslation<'wasm> {
    pub module: TranslatedModule<'wasm>,
    pub function_body_inputs: PrimaryMap<DefinedFuncIndex, FunctionBodyInput<'wasm>>,
    pub types: PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
}

pub struct FunctionBodyInput<'wasm> {
    pub validator: FuncToValidate<ValidatorResources>,
    pub body: FunctionBody<'wasm>,
}

impl<'a, 'wasm> TypeConvert for ModuleEnvironment<'a, 'wasm> {
    fn lookup_heap_type(&self, _index: UnpackedIndex) -> WasmHeapType {
        todo!()
    }
}

impl<'a, 'wasm> ModuleEnvironment<'a, 'wasm> {
    pub fn new(validator: &'a mut Validator) -> Self {
        Self {
            result: ModuleTranslation::default(),
            validator,
        }
    }

    /// Marks a given function as "escaped" i.e. accessible outside of this module
    fn mark_func_as_escaped(&mut self, func_index: FuncIndex) {
        let ty = &mut self.result.module.functions[func_index];
        if ty.is_escaping() {
            return;
        }
        let index = self.result.module.num_escaped_funcs as u32;
        ty.func_ref = FuncRefIndex::from_u32(index);
        self.result.module.num_escaped_funcs += 1;
    }

    pub fn translate(
        mut self,
        parser: Parser,
        data: &'wasm [u8],
    ) -> WasmResult<ModuleTranslation<'wasm>> {
        for payload in parser.parse_all(data) {
            self.translate_payload(payload?)?;
        }

        Ok(self.result)
    }

    pub fn translate_payload(&mut self, payload: Payload<'wasm>) -> WasmResult<()> {
        match payload {
            Payload::Version {
                num,
                encoding,
                range,
            } => {
                self.validator.version(num, encoding, &range)?;
                assert!(matches!(encoding, Encoding::Module), "expected core module");
            }
            Payload::TypeSection(types) => {
                self.validator.type_section(&types)?;
                self.result.types.reserve_exact(types.count() as usize);
                self.result
                    .module
                    .types
                    .reserve_exact(types.count() as usize);

                for ty in types.into_iter_err_on_gc_types() {
                    let wasm_func_type = self.convert_func_type(&ty?);
                    let idx = self.result.types.push(WasmSubType {
                        composite_type: WasmCompositeType::Func(wasm_func_type),
                    });
                    self.result.module.types.push(idx);
                }
            }
            Payload::ImportSection(imports) => {
                self.validator.import_section(&imports)?;
                self.result
                    .module
                    .imports
                    .reserve_exact(imports.count() as usize);

                for import in imports {
                    let import = import?;

                    let index = match import.ty {
                        TypeRef::Func(index) => {
                            self.result.module.num_imported_funcs += 1;

                            let interned_index =
                                self.result.module.types[TypeIndex::from_u32(index)];

                            let func_index = self.result.module.functions.push(FunctionType {
                                signature: interned_index,
                                func_ref: FuncRefIndex::reserved_value(),
                            });

                            EntityIndex::Function(func_index)
                        }
                        TypeRef::Table(ty) => {
                            let table_index = self
                                .result
                                .module
                                .table_plans
                                .push(TablePlan::for_table(self.convert_table_type(&ty)));

                            EntityIndex::Table(table_index)
                        }
                        TypeRef::Memory(ty) => {
                            let memory_index = self
                                .result
                                .module
                                .memory_plans
                                .push(MemoryPlan::for_memory_type(ty));

                            EntityIndex::Memory(memory_index)
                        }
                        TypeRef::Global(ty) => {
                            let global_index = self
                                .result
                                .module
                                .globals
                                .push(self.convert_global_type(&ty));

                            EntityIndex::Global(global_index)
                        }
                        TypeRef::Tag(_) => todo!(),
                    };

                    self.result.module.imports.push(Import {
                        module: import.module,
                        field: import.name,
                        index,
                    })
                }
            }
            Payload::FunctionSection(funcs) => {
                self.validator.function_section(&funcs)?;

                self.result
                    .module
                    .functions
                    .reserve_exact(funcs.count() as usize);

                for func in funcs {
                    let index = TypeIndex::from_u32(func?);
                    let interned_index = self.result.module.types[index];

                    self.result.module.functions.push(FunctionType {
                        signature: interned_index,
                        func_ref: FuncRefIndex::reserved_value(),
                    });
                }
            }
            Payload::TableSection(tables) => {
                self.validator.table_section(&tables)?;
                self.result
                    .module
                    .table_plans
                    .reserve_exact(tables.count() as usize);

                for table in tables {
                    let table = table?;

                    self.result
                        .module
                        .table_plans
                        .push(TablePlan::for_table(self.convert_table_type(&table.ty)));

                    // TODO handle table init
                }
            }
            Payload::MemorySection(memories) => {
                self.validator.memory_section(&memories)?;
                self.result
                    .module
                    .memory_plans
                    .reserve_exact(memories.count() as usize);

                for memory in memories {
                    self.result
                        .module
                        .memory_plans
                        .push(MemoryPlan::for_memory_type(memory?));
                }
            }
            Payload::GlobalSection(globals) => {
                self.validator.global_section(&globals)?;
                self.result
                    .module
                    .globals
                    .reserve_exact(globals.count() as usize);

                for global in globals {
                    let global = global?;

                    let global_index = self
                        .result
                        .module
                        .globals
                        .push(self.convert_global_type(&global.ty));

                    let (expr, escaping_funcs) = ConstExpr::from_wasmparser(global.init_expr)?;

                    for func_index in escaping_funcs {
                        self.mark_func_as_escaped(func_index);
                    }

                    let def_index = self.result.module.global_initializers.push(expr);
                    debug_assert_eq!(
                        Some(def_index),
                        self.result.module.defined_global_index(global_index)
                    )
                }
            }
            Payload::ExportSection(exports) => {
                self.validator.export_section(&exports)?;
                // TODO reserve size

                for export in exports {
                    let export = export?;
                    let index = match export.kind {
                        ExternalKind::Func => {
                            let func_idx = FuncIndex::from_u32(export.index);
                            self.mark_func_as_escaped(func_idx);
                            EntityIndex::Function(func_idx)
                        }
                        ExternalKind::Table => {
                            EntityIndex::Table(TableIndex::from_u32(export.index))
                        }
                        ExternalKind::Memory => {
                            EntityIndex::Memory(MemoryIndex::from_u32(export.index))
                        }
                        ExternalKind::Global => {
                            EntityIndex::Global(GlobalIndex::from_u32(export.index))
                        }
                        ExternalKind::Tag => todo!(),
                    };

                    self.result.module.exports.insert(export.name, index);
                }
            }
            Payload::StartSection { func, range } => {
                self.validator.start_section(func, &range)?;
                let func_index = FuncIndex::from_u32(func);
                self.mark_func_as_escaped(func_index);
                debug_assert!(self.result.module.start_func.is_none());

                self.result.module.start_func = Some(func_index);
            }
            Payload::ElementSection(elements) => {
                self.validator.element_section(&elements)?;
                todo!()
            }
            Payload::DataCountSection { count, range } => {
                self.validator.data_count_section(count, &range)?;
                todo!()
            }
            Payload::DataSection(section) => {
                self.validator.data_section(&section)?;
                todo!()
            }
            Payload::CodeSectionStart { count, range, .. } => {
                self.validator.code_section_start(count, &range)?;
                self.result
                    .function_body_inputs
                    .reserve_exact(count as usize);
            }
            Payload::CodeSectionEntry(mut body) => {
                let validator = self.validator.code_section_entry(&body)?;

                body.allow_memarg64(self.validator.features().contains(WasmFeatures::MEMORY64));
                self.result
                    .function_body_inputs
                    .push(FunctionBodyInput { validator, body });
            }

            // TODO name section & dwarf debug section
            Payload::CustomSection(s) if s.name() == "name" => {}
            Payload::CustomSection(s) if s.name().starts_with(".debug_") => {}
            Payload::CustomSection(s) if s.name() == "producers" => {}
            Payload::CustomSection(s) if s.name() == "target_features" => {}
            Payload::End(_) => {}
            other => {
                self.validator.payload(&other)?;
                panic!("unimplemented section {:?}", other);
            }
        }

        Ok(())
    }
}
