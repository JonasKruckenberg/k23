use crate::runtime::compile::FuncCompileInput;
use crate::runtime::errors::TranslationError;
use crate::runtime::translate::{
    FunctionType, Import, MemoryInitializer, MemoryPlan, ProducersLanguage, ProducersLanguageField,
    ProducersSdk, ProducersSdkField, ProducersTool, ProducersToolField, TableInitialValue,
    TablePlan, TableSegment, TableSegmentElements, Translation,
};
use crate::runtime::vmcontext::FuncRefIndex;
use alloc::sync::Arc;
use alloc::vec::Vec;
use cranelift_codegen::entity::Unsigned;
use cranelift_entity::packed_option::ReservedValue;
use cranelift_wasm::wasmparser::{
    BinaryReader, CustomSectionReader, DataKind, ElementItems, ElementKind, Encoding, ExternalKind,
    Operator, Parser, Payload, ProducersFieldValue, ProducersSectionReader, TableInit, TypeRef,
    UnpackedIndex, Validator, ValidatorResources, WasmFeatures,
};
use cranelift_wasm::{
    ConstExpr, EntityIndex, FuncIndex, GlobalIndex, MemoryIndex, TableIndex, TypeConvert,
    TypeIndex, WasmCompositeType, WasmHeapType, WasmSubType,
};
use object::Bytes;

pub struct ModuleEnvironment<'a, 'wasm> {
    result: Translation<'wasm>,
    validator: &'a mut Validator,
}

impl TypeConvert for ModuleEnvironment<'_, '_> {
    fn lookup_heap_type(&self, _index: UnpackedIndex) -> WasmHeapType {
        todo!()
    }

    fn lookup_type_index(&self, _index: UnpackedIndex) -> cranelift_wasm::EngineOrModuleTypeIndex {
        todo!()
    }
}

impl<'a, 'wasm> ModuleEnvironment<'a, 'wasm> {
    pub fn new(validator: &'a mut Validator) -> Self {
        Self {
            result: Translation::default(),
            validator,
        }
    }

    fn mark_function_as_escaped(&mut self, func_index: FuncIndex) {
        let ty = &mut self.result.module.functions[func_index];
        if ty.is_escaping() {
            return;
        }
        let index = self.result.module.num_escaped_funcs;
        ty.func_ref = FuncRefIndex::from_u32(index);
        self.result.module.num_escaped_funcs += 1;
    }

    pub fn translate(
        mut self,
        parser: Parser,
        data: &'wasm [u8],
    ) -> Result<Translation<'wasm>, TranslationError> {
        for payload in parser.parse_all(data) {
            self.translate_payload(payload?)?;
        }

        Ok(self.result)
    }

    #[allow(clippy::too_many_lines)]
    pub fn translate_payload(&mut self, payload: Payload<'wasm>) -> Result<(), TranslationError> {
        log::trace!("Translating payload section {payload:?}");
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
                self.result
                    .module
                    .types
                    .reserve_exact(types.count() as usize);
                self.result.types.reserve_exact(types.count() as usize);

                for ty in types.into_iter_err_on_gc_types() {
                    let wasm_func_type = self.convert_func_type(&ty?);
                    let idx = self.result.types.push(WasmSubType {
                        is_final: true,
                        supertype: None,
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
                            self.result.module.num_imported_functions += 1;

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
                                .push(TablePlan::for_table(self.convert_table_type(&ty)?));

                            EntityIndex::Table(table_index)
                        }
                        TypeRef::Memory(ty) => {
                            let memory_index = self
                                .result
                                .module
                                .memory_plans
                                .push(MemoryPlan::for_memory(ty));

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
                        name: import.name,
                        ty: index,
                    });
                }
            }
            Payload::FunctionSection(functions) => {
                self.validator.function_section(&functions)?;
                self.result
                    .module
                    .functions
                    .reserve_exact(functions.count() as usize);

                for function in functions {
                    let index = TypeIndex::from_u32(function?);
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
                        .push(TablePlan::for_table(self.convert_table_type(&table.ty)?));

                    let init = match table.init {
                        TableInit::RefNull => TableInitialValue::RefNull,
                        TableInit::Expr(expr) => {
                            let (expr, escaped) = ConstExpr::from_wasmparser(expr)?;
                            for f in escaped {
                                self.mark_function_as_escaped(f);
                            }
                            TableInitialValue::ConstExpr(expr)
                        }
                    };
                    self.result
                        .module
                        .table_initializers
                        .initial_values
                        .push(init);
                }
            }
            Payload::MemorySection(memories) => {
                self.validator.memory_section(&memories)?;
                self.result
                    .module
                    .memory_plans
                    .reserve_exact(memories.count() as usize);

                for ty in memories {
                    let ty = ty?;

                    assert!(ty.page_size_log2.is_none());
                    self.result
                        .module
                        .memory_plans
                        .push(MemoryPlan::for_memory(ty));
                }
            }
            Payload::TagSection(tags) => {
                self.validator.tag_section(&tags)?;
                todo!()
            }
            Payload::GlobalSection(globals) => {
                self.validator.global_section(&globals)?;
                self.result
                    .module
                    .globals
                    .reserve_exact(globals.count() as usize);

                for global in globals {
                    let global = global?;

                    self.result
                        .module
                        .globals
                        .push(self.convert_global_type(&global.ty));

                    let (expr, escaped) = ConstExpr::from_wasmparser(global.init_expr)?;
                    for func in escaped {
                        self.mark_function_as_escaped(func);
                    }
                    self.result.module.global_initializers.push(expr);
                }
            }
            Payload::ExportSection(exports) => {
                self.validator.export_section(&exports)?;
                self.result.module.exports.reserve(exports.count() as usize);

                for export in exports {
                    let export = export?;
                    let index = match export.kind {
                        ExternalKind::Func => {
                            let func_idx = FuncIndex::from_u32(export.index);
                            self.mark_function_as_escaped(func_idx);
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
                self.result.module.start = Some(FuncIndex::from_u32(func));
            }
            Payload::ElementSection(elements) => {
                self.validator.element_section(&elements)?;

                for element in elements {
                    let element = element?;

                    let elements = match element.items {
                        ElementItems::Functions(funcs) => {
                            let mut out = Vec::with_capacity(funcs.count() as usize);
                            for func in funcs {
                                let func = FuncIndex::from_u32(func?);
                                self.mark_function_as_escaped(func);
                                out.push(func);
                            }
                            TableSegmentElements::Functions(out.into_boxed_slice())
                        }
                        ElementItems::Expressions(_, exprs) => {
                            let mut out = Vec::with_capacity(exprs.count() as usize);
                            for expr in exprs {
                                let (expr, escaped) = ConstExpr::from_wasmparser(expr?)?;
                                for func in escaped {
                                    self.mark_function_as_escaped(func);
                                }
                                out.push(expr);
                            }
                            TableSegmentElements::Expressions(out.into_boxed_slice())
                        }
                    };

                    match element.kind {
                        ElementKind::Passive => {
                            self.result.module.passive_element_segments.push(elements);
                        }
                        ElementKind::Active {
                            table_index,
                            offset_expr,
                        } => {
                            let table_index = TableIndex::from_u32(table_index.unwrap_or(0));
                            let mut offset_expr_reader = offset_expr.get_binary_reader();
                            let (base, offset) = match offset_expr_reader.read_operator()? {
                                Operator::I32Const { value } => (None, value.unsigned()),
                                Operator::GlobalGet { global_index } => {
                                    (Some(GlobalIndex::from_u32(global_index)), 0)
                                }
                                ref s => {
                                    panic!("unsupported init expr in element section: {:?}", s);
                                }
                            };

                            self.result
                                .module
                                .table_initializers
                                .segments
                                .push(TableSegment {
                                    table_index,
                                    base,
                                    offset,
                                    elements,
                                });
                        }
                        ElementKind::Declared => {}
                    }
                }
            }
            Payload::DataCountSection { count, range } => {
                self.validator.data_count_section(count, &range)?;
            }
            Payload::DataSection(section) => {
                self.validator.data_section(&section)?;

                for data in section {
                    let data = data?;
                    match data.kind {
                        DataKind::Passive => {
                            self.result.module.passive_data_segments.push(data.data);
                        }
                        DataKind::Active {
                            memory_index,
                            offset_expr,
                        } => {
                            let memory_index = MemoryIndex::from_u32(memory_index);
                            let mut offset_expr_reader = offset_expr.get_binary_reader();
                            let (base, offset) = match offset_expr_reader.read_operator()? {
                                Operator::I32Const { value } => (None, value.unsigned()),
                                Operator::GlobalGet { global_index } => {
                                    (Some(GlobalIndex::from_u32(global_index)), 0)
                                }
                                ref s => {
                                    panic!("unsupported init expr in element section: {:?}", s);
                                }
                            };

                            self.result.module.memory_initializers.runtime.push(
                                MemoryInitializer {
                                    memory_index,
                                    base,
                                    offset,
                                    bytes: data.data,
                                },
                            );
                        }
                    }
                }
            }
            Payload::CodeSectionStart { count, range, .. } => {
                self.validator.code_section_start(count, &range)?;
                self.result
                    .func_compile_inputs
                    .reserve_exact(count as usize);
            }
            Payload::CodeSectionEntry(body) => {
                let validator = self.validator.code_section_entry(&body)?;

                self.result
                    .func_compile_inputs
                    .push(FuncCompileInput { body, validator });
            }
            // Payload::CustomSection(sec) if sec.name() == "name" => {
            //     self.parse_name_section(NameSectionReader::new(sec.data(), sec.data_offset()))?;
            // }
            Payload::CustomSection(sec) if sec.name() == "producers" => {
                let reader = ProducersSectionReader::new(BinaryReader::new(
                    sec.data(),
                    sec.data_offset(),
                    *self.validator.features(),
                ))?;

                self.parse_producers_section(reader)?;
            }
            Payload::CustomSection(sec) if sec.name() == "target_features" => {
                self.parse_target_feature_section(&sec);
            }
            Payload::CustomSection(sec) => {
                let name = sec.name().trim_end_matches(".dwo");
                if !name.starts_with(".debug_") {
                    log::debug!("unhandled custom section {sec:?}");
                    return Ok(());
                }
                self.parse_dwarf_section(name, &sec);
            }
            Payload::End(_) => {}
            section => log::warn!("Unknown section {section:?}"),
        }

        Ok(())
    }

    // fn parse_name_section(&mut self, reader: NameSectionReader<'wasm>) -> WasmResult<()> {
    //     for subsection in reader {
    //         match subsection? {
    //             Name::Module { name, .. } => {
    //                 self.result.module.debug_info.names.module_name = Some(name);
    //             }
    //             Name::Function(names) => {
    //                 for name in names {
    //                     let name = name?;
    //
    //                     // Skip this naming if it's naming a function that
    //                     // doesn't actually exist.
    //                     if (name.index as usize) >= self.result.module.functions.len() {
    //                         continue;
    //                     }
    //
    //                     self.result
    //                         .module
    //                         .debug_info
    //                         .names
    //                         .func_names
    //                         .insert(FuncIndex::from_u32(name.index), name.name);
    //                 }
    //             }
    //             Name::Local(names) => {
    //                 for naming in names {
    //                     let name = naming?;
    //                     let mut result = HashMap::default();
    //
    //                     for name in name.names {
    //                         let name = name?;
    //
    //                         // Skip this naming if it's naming a function that
    //                         // doesn't actually exist.
    //                         if (name.index as usize) >= self.result.module.functions.len() {
    //                             continue;
    //                         }
    //
    //                         result.insert(name.index, name.name);
    //                     }
    //
    //                     self.result
    //                         .module
    //                         .debug_info
    //                         .names
    //                         .locals_names
    //                         .insert(FuncIndex::from_u32(name.index), result);
    //                 }
    //             }
    //             Name::Global(names) => {
    //                 for name in names {
    //                     let name = name?;
    //                     self.result
    //                         .module
    //                         .debug_info
    //                         .names
    //                         .global_names
    //                         .insert(GlobalIndex::from_u32(name.index), name.name);
    //                 }
    //             }
    //             Name::Data(names) => {
    //                 for name in names {
    //                     let name = name?;
    //                     self.result
    //                         .module
    //                         .debug_info
    //                         .names
    //                         .data_names
    //                         .insert(DataIndex::from_u32(name.index), name.name);
    //                 }
    //             }
    //             Name::Label(_) => log::debug!("unused name subsection label"),
    //             Name::Type(_) => log::debug!("unused name subsection type"),
    //             Name::Table(_) => log::debug!("unused name subsection table"),
    //             Name::Memory(_) => log::debug!("unused name subsection memory"),
    //             Name::Element(_) => log::debug!("unused name subsection element"),
    //             Name::Field(_) => log::debug!("unused name subsection field"),
    //             Name::Tag(_) => log::debug!("unused name subsection tag"),
    //             Name::Unknown { .. } => {}
    //         }
    //     }
    //
    //     Ok(())
    // }

    fn parse_target_feature_section(&mut self, section: &CustomSectionReader<'wasm>) {
        let mut bytes = Bytes(section.data());

        let _count: u8 = *bytes.read().unwrap();

        let mut required_features = WasmFeatures::empty();

        while !bytes.is_empty() {
            let prefix: u8 = *bytes.read().unwrap();
            assert_eq!(prefix, 0x2b, "only the `+` prefix is supported");

            let len = bytes.read_uleb128().unwrap();
            let feature = bytes.read_bytes(usize::try_from(len).unwrap()).unwrap();
            let feature = core::str::from_utf8(feature.0).unwrap();

            match feature {
                "atomics" => required_features.insert(WasmFeatures::THREADS),
                "bulk-memory" => required_features.insert(WasmFeatures::BULK_MEMORY),
                "exception-handling" => required_features.insert(WasmFeatures::EXCEPTIONS),
                "multivalue" => required_features.insert(WasmFeatures::MULTI_VALUE),
                "mutable-globals" => required_features.insert(WasmFeatures::MUTABLE_GLOBAL),
                "nontrapping-fptoint" => {
                    required_features.insert(WasmFeatures::SATURATING_FLOAT_TO_INT);
                }
                "sign-ext" => required_features.insert(WasmFeatures::SIGN_EXTENSION),
                "simd128" => required_features.insert(WasmFeatures::SIMD),
                "tail-call" => required_features.insert(WasmFeatures::TAIL_CALL),
                _ => log::warn!("unknown required WASM feature `{feature}`"),
            }
        }

        self.result.module.required_features = required_features;
    }

    fn parse_producers_section(
        &mut self,
        section: ProducersSectionReader<'wasm>,
    ) -> Result<(), TranslationError> {
        for field in section {
            let field = field?;
            match field.name {
                "language" => {
                    for value in field.values {
                        let ProducersFieldValue { name, version } = value?;
                        let name = match name {
                            "wat" => ProducersLanguage::Wat,
                            "C" => ProducersLanguage::C,
                            "C++" => ProducersLanguage::Cpp,
                            "Rust" => ProducersLanguage::Rust,
                            "JavaScript" => ProducersLanguage::JavaScript,
                            _ => ProducersLanguage::Other(name),
                        };

                        self.result
                            .module
                            .debug_info
                            .producers
                            .language
                            .push(ProducersLanguageField { name, version });
                    }
                }
                "processed-by" => {
                    for value in field.values {
                        let ProducersFieldValue { name, version } = value?;
                        let name = match name {
                            "wabt" => ProducersTool::Wabt,
                            "LLVM" => ProducersTool::Llvm,
                            "clang" => ProducersTool::Clang,
                            "lld" => ProducersTool::Lld,
                            "Binaryen" => ProducersTool::Binaryen,
                            "rustc" => ProducersTool::Rustc,
                            "wasm-bindgen" => ProducersTool::WasmBindgen,
                            "wasm-pack" => ProducersTool::WasmPack,
                            "webassemblyjs" => ProducersTool::Webassemblyjs,
                            "wasm-snip" => ProducersTool::WasmSnip,
                            "Javy" => ProducersTool::Javy,
                            _ => ProducersTool::Other(name),
                        };

                        self.result
                            .module
                            .debug_info
                            .producers
                            .processed_by
                            .push(ProducersToolField { name, version });
                    }
                }
                "sdk" => {
                    for value in field.values {
                        let ProducersFieldValue { name, version } = value?;
                        let name = match name {
                            "Emscripten" => ProducersSdk::Emscripten,
                            "Webpack" => ProducersSdk::Webpack,
                            _ => ProducersSdk::Other(name),
                        };

                        self.result
                            .module
                            .debug_info
                            .producers
                            .sdk
                            .push(ProducersSdkField { name, version });
                    }
                }
                _ => unreachable!(),
            }
        }

        Ok(())
    }

    fn parse_dwarf_section(&mut self, name: &'wasm str, section: &CustomSectionReader<'wasm>) {
        let endian = gimli::LittleEndian;
        let data = section.data();
        let slice = gimli::EndianSlice::new(data, endian);

        let mut dwarf = gimli::Dwarf::default();
        let info = &mut self.result.module.debug_info;

        match name {
            // `gimli::Dwarf` fields.
            ".debug_abbrev" => dwarf.debug_abbrev = gimli::DebugAbbrev::new(data, endian),
            ".debug_addr" => dwarf.debug_addr = gimli::DebugAddr::from(slice),
            ".debug_info" => {
                dwarf.debug_info = gimli::DebugInfo::new(data, endian);
            }
            ".debug_line" => dwarf.debug_line = gimli::DebugLine::new(data, endian),
            ".debug_line_str" => dwarf.debug_line_str = gimli::DebugLineStr::from(slice),
            ".debug_str" => dwarf.debug_str = gimli::DebugStr::new(data, endian),
            ".debug_str_offsets" => dwarf.debug_str_offsets = gimli::DebugStrOffsets::from(slice),
            ".debug_str_sup" => {
                let dwarf_sup = gimli::Dwarf {
                    debug_str: gimli::DebugStr::from(slice),
                    ..Default::default()
                };
                dwarf.sup = Some(Arc::new(dwarf_sup));
            }
            ".debug_types" => dwarf.debug_types = gimli::DebugTypes::from(slice),

            // Additional fields.
            ".debug_loc" => info.debug_loc = gimli::DebugLoc::from(slice),
            ".debug_loclists" => info.debug_loclists = gimli::DebugLocLists::from(slice),
            ".debug_ranges" => info.debug_ranges = gimli::DebugRanges::new(data, endian),
            ".debug_rnglists" => info.debug_rnglists = gimli::DebugRngLists::new(data, endian),

            // DWARF package fields
            ".debug_cu_index" => info.debug_cu_index = gimli::DebugCuIndex::new(data, endian),
            ".debug_tu_index" => info.debug_tu_index = gimli::DebugTuIndex::new(data, endian),

            // We don't use these at the moment.
            ".debug_aranges" | ".debug_pubnames" | ".debug_pubtypes" => return,
            other => {
                log::warn!("unknown debug section `{}`", other);
                return;
            }
        }

        dwarf.ranges = gimli::RangeLists::new(info.debug_ranges, info.debug_rnglists);
        dwarf.locations = gimli::LocationLists::new(info.debug_loc, info.debug_loclists);

        info.dwarf = dwarf;
    }
}
