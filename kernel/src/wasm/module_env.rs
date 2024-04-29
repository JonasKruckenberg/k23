use crate::wasm::module::{FunctionType, Import, MemoryPlan, Module, TablePlan};
use alloc::boxed::Box;
use core::mem;
use cranelift_codegen::entity::PrimaryMap;
use cranelift_wasm::wasmparser::{FuncValidator, FunctionBody, UnpackedIndex, ValidatorResources};
use cranelift_wasm::{
    ConstExpr, DataIndex, DefinedFuncIndex, ElemIndex, EntityIndex, FuncIndex, Global, GlobalIndex,
    Memory, MemoryIndex, ModuleInternedTypeIndex, Table, TableIndex, TypeConvert, TypeIndex,
    WasmCompositeType, WasmFuncType, WasmHeapType, WasmResult, WasmSubType,
};

#[derive(Default)]
pub struct ModuleTranslation<'wasm> {
    pub module: Module<'wasm>,
    pub function_body_inputs: PrimaryMap<DefinedFuncIndex, FunctionBodyInput<'wasm>>,
    pub types: PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
}

pub struct FunctionBodyInput<'wasm> {
    pub validator: FuncValidator<ValidatorResources>,
    pub body: FunctionBody<'wasm>,
}

pub struct ModuleEnvironment<'wasm> {
    result: ModuleTranslation<'wasm>,
}

impl<'wasm> ModuleEnvironment<'wasm> {
    pub fn new() -> Self {
        Self {
            result: ModuleTranslation::default(),
        }
    }

    pub fn finish(&mut self) -> ModuleTranslation<'wasm> {
        mem::take(&mut self.result)
    }
}

impl<'wasm> TypeConvert for ModuleEnvironment<'wasm> {
    fn lookup_heap_type(&self, index: UnpackedIndex) -> WasmHeapType {
        todo!()
    }
}

impl<'wasm> cranelift_wasm::ModuleEnvironment<'wasm> for ModuleEnvironment<'wasm> {
    fn reserve_types(&mut self, num: u32) -> WasmResult<()> {
        self.result.module.types.reserve_exact(num as usize);
        Ok(())
    }
    fn reserve_func_types(&mut self, num: u32) -> WasmResult<()> {
        self.result.module.functions.reserve_exact(num as usize);
        Ok(())
    }
    fn reserve_tables(&mut self, num: u32) -> WasmResult<()> {
        self.result.module.table_plans.reserve_exact(num as usize);
        Ok(())
    }
    fn reserve_memories(&mut self, num: u32) -> WasmResult<()> {
        self.result.module.memory_plans.reserve_exact(num as usize);
        Ok(())
    }
    fn reserve_globals(&mut self, num: u32) -> WasmResult<()> {
        self.result.module.globals.reserve_exact(num as usize);
        Ok(())
    }
    fn reserve_exports(&mut self, _num: u32) -> WasmResult<()> {
        // TODO reserve space?
        Ok(())
    }
    fn reserve_imports(&mut self, num: u32) -> WasmResult<()> {
        self.result.module.imports.reserve_exact(num as usize);
        Ok(())
    }
    fn reserve_function_bodies(&mut self, bodies: u32, _code_section_offset: u64) {
        self.result
            .function_body_inputs
            .reserve_exact(bodies as usize);
    }

    fn declare_type_func(&mut self, wasm_func_type: WasmFuncType) -> WasmResult<()> {
        let idx = self.result.types.push(WasmSubType {
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
        self.result.module.num_imported_funcs += 1;

        let interned_index = self.result.module.types[index];

        let func_index = self.result.module.functions.push(FunctionType {
            signature: interned_index,
        });

        self.result.module.imports.push(Import {
            module,
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

        let table_index = self.result.module.table_plans.push(TablePlan { table });

        self.result.module.imports.push(Import {
            module,
            field,
            index: EntityIndex::Table(table_index),
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

        let memory_index = self.result.module.memory_plans.push(MemoryPlan { memory });

        self.result.module.imports.push(Import {
            module,
            field,
            index: EntityIndex::Memory(memory_index),
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

        let global_index = self.result.module.globals.push(global);

        self.result.module.imports.push(Import {
            module,
            field,
            index: EntityIndex::Global(global_index),
        });

        Ok(())
    }

    fn declare_func_type(&mut self, index: TypeIndex) -> WasmResult<()> {
        let interned_index = self.result.module.types[index];

        self.result.module.functions.push(FunctionType {
            signature: interned_index,
        });

        Ok(())
    }

    fn declare_table(&mut self, table: Table) -> WasmResult<()> {
        self.result.module.table_plans.push(TablePlan { table });

        Ok(())
    }

    fn declare_memory(&mut self, memory: Memory) -> WasmResult<()> {
        self.result.module.memory_plans.push(MemoryPlan { memory });

        Ok(())
    }

    fn declare_global(&mut self, global: Global, _init: ConstExpr) -> WasmResult<()> {
        self.result.module.globals.push(global);

        // TODO handle global initializer

        Ok(())
    }

    fn declare_func_export(&mut self, func_index: FuncIndex, name: &'wasm str) -> WasmResult<()> {
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
        self.result.module.start = Some(index);
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
    }

    fn declare_passive_element(
        &mut self,
        index: ElemIndex,
        elements: Box<[FuncIndex]>,
    ) -> WasmResult<()> {
        todo!()
    }

    fn declare_passive_data(&mut self, data_index: DataIndex, data: &'wasm [u8]) -> WasmResult<()> {
        todo!()
    }

    fn define_function_body(
        &mut self,
        validator: FuncValidator<ValidatorResources>,
        body: FunctionBody<'wasm>,
    ) -> WasmResult<()> {
        self.result
            .function_body_inputs
            .push(FunctionBodyInput { validator, body });

        Ok(())
    }

    fn declare_data_initialization(
        &mut self,
        memory_index: MemoryIndex,
        base: Option<GlobalIndex>,
        offset: u64,
        data: &'wasm [u8],
    ) -> WasmResult<()> {
        todo!()
    }
}
