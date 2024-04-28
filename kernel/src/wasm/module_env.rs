use crate::wasm::module::{
    FuncRefIndex, FunctionType, Import, MemoryPlan, Module, TablePlan, TableSegmentElements,
};
use crate::wasm::{MEMORY_GUARD_SIZE, WASM32_MAX_PAGES, WASM64_MAX_PAGES};
use alloc::borrow::{Cow, ToOwned};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::Formatter;
use core::{fmt, mem};
use cranelift_codegen::entity::PrimaryMap;
use cranelift_codegen::packed_option::ReservedValue;
use cranelift_wasm::wasmparser::{
    CustomSectionReader, Encoding, ExternalKind, FuncToValidate, FuncValidator, FunctionBody, Name,
    NameSectionReader, Naming, Operator, Parser, Payload, RefType, TableInit, TypeRef,
    UnpackedIndex, Validator, ValidatorResources, WasmFeatures,
};
use cranelift_wasm::{
    wasmparser, ConstExpr, DataIndex, DefinedFuncIndex, ElemIndex, EntityIndex, EntityType,
    FuncIndex, Global, GlobalIndex, Memory, MemoryIndex, ModuleInternedTypeIndex, Table,
    TableIndex, TypeConvert, TypeIndex, WasmCompositeType, WasmError, WasmFuncType, WasmHeapType,
    WasmRefType, WasmResult, WasmSubType, WasmValType,
};
use hashbrown::HashMap;

#[derive(Default)]
pub struct ModuleTranslation<'wasm> {
    pub module: Module<'wasm>,

    /// Raw wasm bytes
    wasm: &'wasm [u8],

    /// References to the function bodies.
    pub function_body_inputs: PrimaryMap<DefinedFuncIndex, FunctionBodyData<'wasm>>,

    /// DWARF debug information, if enabled, parsed from the module.
    pub debuginfo: DebugInfoData<'wasm>,

    /// When we're parsing the code section this will be incremented so we know
    /// which function is currently being defined.
    code_index: u32,

    /// Total size of all data pushed onto `data` so far.
    total_data: u32,

    /// List of data segments found in this module which should be concatenated
    /// together for the final compiled artifact.
    ///
    /// These data segments, when concatenated, are indexed by the
    /// `MemoryInitializer` type.
    pub data: Vec<Cow<'wasm, [u8]>>,

    /// List of passive element segments found in this module which will get
    /// concatenated for the final artifact.
    pub passive_data: Vec<&'wasm [u8]>,

    /// Total size of all passive data pushed into `passive_data` so far.
    total_passive_data: u32,
}

impl<'a> fmt::Debug for ModuleTranslation<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModuleTranslation")
            .field("module", &self.module)
            .field("debuginfo", &self.debuginfo)
            .finish()
    }
}

/// Contains function data: byte code and its offset in the module.
pub struct FunctionBodyData<'a> {
    /// The body of the function, containing code and locals.
    pub body: FunctionBody<'a>,
    /// Validator for the function body
    pub validator: FuncValidator<ValidatorResources>,
}

#[derive(Debug, Default)]
#[allow(missing_docs)]
pub struct DebugInfoData<'a> {
    pub dwarf: Dwarf<'a>,
    pub name_section: NameSection<'a>,
    pub wasm_file: WasmFileInfo<'a>,
    pub debug_loc: gimli::DebugLoc<Reader<'a>>,
    pub debug_loclists: gimli::DebugLocLists<Reader<'a>>,
    pub debug_ranges: gimli::DebugRanges<Reader<'a>>,
    pub debug_rnglists: gimli::DebugRngLists<Reader<'a>>,
    pub debug_cu_index: gimli::DebugCuIndex<Reader<'a>>,
    pub debug_tu_index: gimli::DebugTuIndex<Reader<'a>>,
}

#[allow(missing_docs)]
pub type Dwarf<'input> = gimli::Dwarf<Reader<'input>>;

type Reader<'input> = gimli::EndianSlice<'input, gimli::LittleEndian>;

#[derive(Debug, Default)]
#[allow(missing_docs)]
pub struct NameSection<'a> {
    pub module_name: Option<&'a str>,
    pub func_names: HashMap<FuncIndex, &'a str>,
    pub locals_names: HashMap<FuncIndex, HashMap<u32, &'a str>>,
}

#[derive(Debug, Default)]
#[allow(missing_docs)]
pub struct WasmFileInfo<'a> {
    pub path: Option<&'a str>,
    pub code_section_offset: u64,
    pub imported_func_count: u32,
    pub funcs: Vec<FunctionMetadata>,
}

#[derive(Debug)]
#[allow(missing_docs)]
pub struct FunctionMetadata {
    pub params: Box<[WasmValType]>,
    pub locals: Box<[(u32, WasmValType)]>,
}

pub struct ModuleEnvironment<'wasm> {
    /// The current module being translated
    result: ModuleTranslation<'wasm>,
    types: PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
}

impl<'wasm> ModuleEnvironment<'wasm> {
    pub fn new() -> Self {
        Self {
            result: ModuleTranslation::default(),
            types: PrimaryMap::new(),
        }
    }

    pub fn finish(
        &mut self,
    ) -> (
        ModuleTranslation<'wasm>,
        PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
    ) {
        let result = mem::take(&mut self.result);
        let types = mem::take(&mut self.types);

        (result, types)
    }

    pub fn flag_func_escaped(&mut self, func_index: FuncIndex) {
        let ty = &mut self.result.module.functions[func_index];
        // If this was already assigned a funcref index no need to re-assign it.
        if ty.is_escaping() {
            return;
        }
        let index = self.result.module.num_escaped_funcs as u32;
        ty.func_ref = FuncRefIndex::from_u32(index);
        self.result.module.num_escaped_funcs += 1;
    }

    fn register_name_section(&mut self, names: NameSectionReader<'wasm>) -> WasmResult<()> {
        for subsection in names {
            match subsection? {
                Name::Module { name, name_range } => {
                    self.result.module.name = Some(name);
                    self.result.debuginfo.name_section.module_name = Some(name);
                }
                Name::Function(names) => {
                    for name in names {
                        let Naming { index, name } = name?;
                        // Skip this naming if it's naming a function that
                        // doesn't actually exist.
                        if (index as usize) >= self.result.module.functions.len() {
                            continue;
                        }

                        // Store the name unconditionally, regardless of
                        // whether we're parsing debuginfo, since function
                        // names are almost always present in the
                        // final compilation artifact.
                        let index = FuncIndex::from_u32(index);
                        self.result
                            .debuginfo
                            .name_section
                            .func_names
                            .insert(index, name);
                    }
                }
                Name::Local(reader) => {
                    for f in reader {
                        let f = f?;
                        // Skip this naming if it's naming a function that
                        // doesn't actually exist.
                        if (f.index as usize) >= self.result.module.functions.len() {
                            continue;
                        }
                        for name in f.names {
                            let Naming { index, name } = name?;

                            self.result
                                .debuginfo
                                .name_section
                                .locals_names
                                .entry(FuncIndex::from_u32(f.index))
                                .or_insert(HashMap::new())
                                .insert(index, name);
                        }
                    }
                }
                Name::Label(_)
                | Name::Type(_)
                | Name::Table(_)
                | Name::Memory(_)
                | Name::Global(_)
                | Name::Element(_)
                | Name::Data(_)
                | Name::Field(_)
                | Name::Tag(_)
                | Name::Unknown { .. } => {}
            }
        }

        Ok(())
    }

    fn register_dwarf_section(&mut self, name: &'wasm str, data: &'wasm [u8]) {
        let name = name.trim_end_matches(".dwo");
        if !name.starts_with(".debug_") {
            return;
        }

        let info = &mut self.result.debuginfo;
        let dwarf = &mut info.dwarf;
        let endian = gimli::LittleEndian;

        let slice = gimli::EndianSlice::new(data, endian);

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
                let mut dwarf_sup: Dwarf<'wasm> = Default::default();
                dwarf_sup.debug_str = gimli::DebugStr::from(slice);
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
    }
}

impl<'wasm> TypeConvert for ModuleEnvironment<'wasm> {
    fn lookup_heap_type(&self, index: UnpackedIndex) -> WasmHeapType {
        todo!()
    }
}

impl<'wasm> cranelift_wasm::ModuleEnvironment<'wasm> for ModuleEnvironment<'wasm> {
    fn reserve_function_bodies(&mut self, bodies: u32, code_section_offset: u64) {
        self.result
            .function_body_inputs
            .reserve_exact(usize::try_from(bodies).unwrap());
        self.result.debuginfo.wasm_file.code_section_offset = code_section_offset;
    }

    fn reserve_tables(&mut self, num: u32) -> WasmResult<()> {
        self.result
            .module
            .table_plans
            .reserve_exact(usize::try_from(num).unwrap());
        Ok(())
    }

    fn reserve_memories(&mut self, num: u32) -> WasmResult<()> {
        self.result
            .module
            .memory_plans
            .reserve_exact(usize::try_from(num).unwrap());
        Ok(())
    }

    fn reserve_globals(&mut self, num: u32) -> WasmResult<()> {
        self.result
            .module
            .globals
            .reserve_exact(usize::try_from(num).unwrap());
        Ok(())
    }

    fn reserve_types(&mut self, num: u32) -> WasmResult<()> {
        self.result
            .module
            .types
            .reserve_exact(usize::try_from(num).unwrap());
        self.types.reserve_exact(usize::try_from(num).unwrap());
        Ok(())
    }

    fn reserve_func_types(&mut self, num: u32) -> WasmResult<()> {
        self.result
            .module
            .functions
            .reserve_exact(usize::try_from(num).unwrap());
        Ok(())
    }

    fn declare_type_func(&mut self, wasm_func_type: WasmFuncType) -> WasmResult<()> {
        let idx = self.types.push(WasmSubType {
            composite_type: WasmCompositeType::Func(wasm_func_type),
        });
        self.result.module.types.push(idx);

        Ok(())
    }

    fn declare_func_import(
        &mut self,
        index: TypeIndex,
        module: &'wasm str,
        field: &'wasm str,
    ) -> WasmResult<()> {
        let interned_index = self.result.module.types[index];
        self.result.module.num_imported_funcs += 1;
        self.result.debuginfo.wasm_file.imported_func_count += 1;

        let func_index = self.result.module.functions.push(FunctionType {
            signature: interned_index,
            func_ref: FuncRefIndex::reserved_value(),
        });

        // Imported functions can escape; in fact, they've already done
        // so to get here.
        self.flag_func_escaped(func_index);

        self.result.module.imports.push(Import {
            name: module,
            field,
            index: EntityIndex::Function(func_index),
        });

        Ok(())
    }

    fn declare_table_import(
        &mut self,
        table: Table,
        module: &'wasm str,
        field: &'wasm str,
    ) -> WasmResult<()> {
        self.result.module.num_imported_tables += 1;
        let plan = TablePlan::for_table(table);
        let index = EntityIndex::Table(self.result.module.table_plans.push(plan));

        self.result.module.imports.push(Import {
            name: module,
            field,
            index,
        });

        Ok(())
    }

    fn declare_memory_import(
        &mut self,
        memory: Memory,
        module: &'wasm str,
        field: &'wasm str,
    ) -> WasmResult<()> {
        self.result.module.num_imported_memories += 1;

        let plan = MemoryPlan::for_memory(memory);
        let index = EntityIndex::Memory(self.result.module.memory_plans.push(plan));

        self.result.module.imports.push(Import {
            name: module,
            field,
            index,
        });

        Ok(())
    }

    fn declare_global_import(
        &mut self,
        global: Global,
        module: &'wasm str,
        field: &'wasm str,
    ) -> WasmResult<()> {
        self.result.module.num_imported_globals += 1;
        let index = EntityIndex::Global(self.result.module.globals.push(global));

        self.result.module.imports.push(Import {
            name: module,
            field,
            index,
        });

        Ok(())
    }

    fn declare_func_type(&mut self, index: TypeIndex) -> WasmResult<()> {
        let interned_index = self.result.module.types[index];

        self.result.module.functions.push(FunctionType {
            signature: interned_index,
            func_ref: FuncRefIndex::reserved_value(),
        });

        Ok(())
    }

    fn declare_table(&mut self, table: Table) -> WasmResult<()> {
        let plan = TablePlan::for_table(table);

        self.result.module.table_plans.push(plan);

        Ok(())
    }

    fn declare_memory(&mut self, memory: Memory) -> WasmResult<()> {
        let plan = MemoryPlan::for_memory(memory);
        self.result.module.memory_plans.push(plan);

        Ok(())
    }

    fn declare_global(&mut self, global: Global, init: ConstExpr) -> WasmResult<()> {
        // todo handle escaped funcs

        self.result.module.globals.push(global);
        self.result.module.global_initializers.push(init);

        Ok(())
    }

    fn declare_func_export(&mut self, func_index: FuncIndex, name: &'wasm str) -> WasmResult<()> {
        self.flag_func_escaped(func_index);
        EntityIndex::Function(func_index);
        self.result
            .module
            .exports
            .insert(name, EntityIndex::Function(func_index));

        Ok(())
    }

    fn declare_table_export(
        &mut self,
        table_index: TableIndex,
        name: &'wasm str,
    ) -> WasmResult<()> {
        self.result
            .module
            .exports
            .insert(name, EntityIndex::Table(table_index));

        Ok(())
    }

    fn declare_memory_export(
        &mut self,
        memory_index: MemoryIndex,
        name: &'wasm str,
    ) -> WasmResult<()> {
        self.result
            .module
            .exports
            .insert(name, EntityIndex::Memory(memory_index));

        Ok(())
    }

    fn declare_global_export(
        &mut self,
        global_index: GlobalIndex,
        name: &'wasm str,
    ) -> WasmResult<()> {
        self.result
            .module
            .exports
            .insert(name, EntityIndex::Global(global_index));

        Ok(())
    }

    fn declare_start_func(&mut self, index: FuncIndex) -> WasmResult<()> {
        self.flag_func_escaped(index);
        debug_assert!(self.result.module.start_func.is_none());
        self.result.module.start_func = Some(index);

        Ok(())
    }

    fn declare_table_elements(
        &mut self,
        table_index: TableIndex,
        base: Option<GlobalIndex>,
        offset: u32,
        elements: Box<[FuncIndex]>,
    ) -> WasmResult<()> {
        todo!()

        // self.result
        //     .module
        //     .table_initialization
        //     .segments
        //     .push(TableSegment {
        //         table_index,
        //         base,
        //         offset,
        //         elements: elements.into(),
        //     });
    }

    fn declare_passive_element(
        &mut self,
        elem_index: ElemIndex,
        elements: Box<[FuncIndex]>,
    ) -> WasmResult<()> {
        let index = self.result.module.passive_elements.len();
        self.result
            .module
            .passive_elements
            .push(TableSegmentElements::Functions(elements));
        self.result
            .module
            .passive_elements_map
            .insert(elem_index, index);

        Ok(())
    }

    fn declare_passive_data(&mut self, data_index: DataIndex, data: &'wasm [u8]) -> WasmResult<()> {
        let range = u32::try_from(data.len())
            .ok()
            .and_then(|size| {
                let start = self.result.total_passive_data;
                let end = start.checked_add(size)?;
                Some(start..end)
            })
            .ok_or_else(|| {
                WasmError::Unsupported("more than 4 gigabytes of data in wasm module".to_string())
            })?;
        self.result.total_passive_data += range.end - range.start;

        self.result.passive_data.push(data);
        self.result
            .module
            .passive_data_map
            .insert(data_index, range);

        Ok(())
    }

    fn define_function_body(
        &mut self,
        validator: FuncValidator<ValidatorResources>,
        mut body: FunctionBody<'wasm>,
    ) -> WasmResult<()> {
        let func_index = self.result.code_index + self.result.module.num_imported_funcs;
        let func_index = FuncIndex::from_u32(func_index);

        // generate_native_debuginfo
        let sig_index = self.result.module.functions[func_index].signature;
        let sig = self.types[sig_index].unwrap_func();

        let mut locals = Vec::new();
        for pair in body.get_locals_reader()? {
            let (cnt, ty) = pair?;
            let ty = self.convert_valtype(ty);
            locals.push((cnt, ty));
        }
        self.result
            .debuginfo
            .wasm_file
            .funcs
            .push(FunctionMetadata {
                locals: locals.into_boxed_slice(),
                params: sig.params().into(),
            });

        // body.allow_memarg64(self.validator.features().contains(WasmFeatures::MEMORY64));

        self.result
            .function_body_inputs
            .push(FunctionBodyData { validator, body });
        self.result.code_index += 1;

        Ok(())
    }

    fn declare_data_initialization(
        &mut self,
        memory_index: MemoryIndex,
        base: Option<GlobalIndex>,
        offset: u64,
        data: &'wasm [u8],
    ) -> WasmResult<()> {
        // let range = mk_range(&mut self.result.total_data)?;
        // initializers.push(MemoryInitializer {
        //     memory_index,
        //     base,
        //     offset,
        //     data: range,
        // });
        self.result.data.push(data.into());

        Ok(())
    }

    fn custom_section(&mut self, name: &'wasm str, data: &'wasm [u8]) -> WasmResult<()> {
        if name == "name" {
            if let Err(e) = self.register_name_section(NameSectionReader::new(data, 0)) {
                log::warn!("failed to parse name section {:?}", e);
            }
        } else if name.starts_with(".debug_") {
            self.register_dwarf_section(name, data)
        }

        Ok(())
    }
}
