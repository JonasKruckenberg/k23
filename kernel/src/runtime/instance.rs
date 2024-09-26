use crate::runtime::compile::CompiledModuleInfo;
use crate::runtime::const_expr::ConstExprEvaluator;
use crate::runtime::guest_memory::CodeMemory;
use crate::runtime::memory::Memory;
use crate::runtime::module::Module;
use crate::runtime::store::Store;
use crate::runtime::table::Table;
use crate::runtime::translate::{TableInitialValue, TableSegmentElements, TranslatedModule};
use crate::runtime::trap::Trap;
use crate::runtime::vmcontext::{
    VMContext, VMContextPlan, VMFuncRef, VMFunctionImport, VMGlobalDefinition, VMGlobalImport,
    VMMemoryDefinition, VMMemoryImport, VMTableDefinition, VMTableImport, VMCONTEXT_MAGIC,
};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt::Formatter;
use core::ptr::NonNull;
use core::{fmt, mem, ptr, slice};
use cranelift_entity::packed_option::ReservedValue;
use cranelift_entity::{entity_impl, EntityRef, PrimaryMap};
use cranelift_wasm::{
    DefinedGlobalIndex, DefinedMemoryIndex, DefinedTableIndex, FuncIndex, GlobalIndex, MemoryIndex,
    ModuleInternedTypeIndex, TableIndex, WasmHeapType,
};

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Instance(u32);
entity_impl!(Instance);

impl Instance {
    pub fn new<'wasm>(store: &mut Store<'wasm>, module: &Module<'wasm>) -> Result<Instance, Trap> {
        let handle = store.allocate_module(module);

        let mut const_eval = ConstExprEvaluator::default();

        log::trace!("initializing vmctx...");
        initialize_vmctx(&mut const_eval, store, handle, &module.info.module)?;
        log::trace!("initialized vmctx...");

        log::trace!("initializing tables...");
        initialize_tables(&mut const_eval, store, handle, &module.info.module)?;
        log::trace!("initialized tables...");

        log::trace!("initializing memories...");
        initialize_memories(store, handle, &module.info.module)?;
        log::trace!("initialized memories...");

        Ok(handle)
    }

    pub fn debug_print_vmctx(self, store: &Store) {
        struct Dbg<'a, 'wasm> {
            data: &'a InstanceData<'wasm>,
        }

        impl<'a, 'wasm> fmt::Debug for Dbg<'a, 'wasm> {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                unsafe {
                    f.debug_struct("VMContext")
                        .field("magic", &self.data.vmctx_magic())
                        .field("tables", &self.data.vmctx_table_definitions())
                        .field("memories", &self.data.vmctx_memory_pointers())
                        .field("owned_memories", &self.data.vmctx_memory_definitions())
                        .field("globals", &self.data.vmctx_global_definitions())
                        .field("func_refs", &self.data.vmctx_func_refs())
                        .field("imported_functions", &self.data.vmctx_function_imports())
                        .field("imported_tables", &self.data.vmctx_table_imports())
                        .field("imported_memories", &self.data.vmctx_memory_imports())
                        .field("imported_globals", &self.data.vmctx_global_imports())
                        .field("stack_limit", &self.data.vmctx_stack_limit())
                        .field("last_wasm_exit_fp", &self.data.vmctx_last_wasm_exit_fp())
                        .field("last_wasm_exit_pc", &self.data.vmctx_last_wasm_exit_pc())
                        .field("last_wasm_entry_sp", &self.data.vmctx_last_wasm_entry_sp())
                        .finish()
                }
            }
        }

        log::debug!(
            "{:#?}",
            Dbg {
                data: &store.instance_data(self)
            }
        );
    }
}

#[allow(unused)]
#[derive(Debug)]
pub struct InstanceData<'wasm> {
    pub module_info: Arc<CompiledModuleInfo<'wasm>>,
    pub code: Arc<CodeMemory>,
    pub vmctx: NonNull<VMContext>,
    pub vmctx_plan: VMContextPlan,
    pub tables: PrimaryMap<DefinedTableIndex, Table>,
    pub memories: PrimaryMap<DefinedMemoryIndex, Memory>,
}

impl<'wasm> InstanceData<'wasm> {
    unsafe fn vmctx_magic(&self) -> u32 {
        *self.vmctx_plus_offset::<u32>(self.vmctx_plan.vmctx_magic())
    }

    unsafe fn vmctx_stack_limit(&self) -> usize {
        *self.vmctx_plus_offset::<usize>(self.vmctx_plan.vmctx_stack_limit())
    }

    unsafe fn vmctx_last_wasm_exit_fp(&self) -> usize {
        *self.vmctx_plus_offset::<usize>(self.vmctx_plan.vmctx_last_wasm_exit_fp())
    }

    unsafe fn vmctx_last_wasm_exit_pc(&self) -> usize {
        *self.vmctx_plus_offset::<usize>(self.vmctx_plan.vmctx_last_wasm_exit_pc())
    }

    unsafe fn vmctx_last_wasm_entry_sp(&self) -> usize {
        *self.vmctx_plus_offset::<usize>(self.vmctx_plan.vmctx_last_wasm_entry_sp())
    }

    unsafe fn vmctx_table_definitions(&self) -> &[VMTableDefinition] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMTableDefinition>(
                self.vmctx_plan.vmctx_table_definitions_start(),
            ),
            self.vmctx_plan.num_defined_tables() as usize,
        )
    }

    unsafe fn vmctx_memory_pointers(&self) -> &[*mut VMMemoryDefinition] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<*mut VMMemoryDefinition>(
                self.vmctx_plan.vmctx_memory_pointers_start(),
            ),
            self.vmctx_plan.num_defined_memories() as usize,
        )
    }

    unsafe fn vmctx_memory_definitions(&self) -> &[VMMemoryDefinition] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMMemoryDefinition>(
                self.vmctx_plan.vmctx_memory_definitions_start(),
            ),
            self.vmctx_plan.num_owned_memories() as usize,
        )
    }

    unsafe fn vmctx_global_definitions(&self) -> &[VMGlobalDefinition] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMGlobalDefinition>(
                self.vmctx_plan.vmctx_global_definitions_start(),
            ),
            self.vmctx_plan.num_defined_globals() as usize,
        )
    }

    unsafe fn vmctx_func_refs(&self) -> &[VMFuncRef] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMFuncRef>(self.vmctx_plan.vmctx_func_refs_start()),
            self.vmctx_plan.num_escaped_funcs() as usize,
        )
    }

    unsafe fn vmctx_function_imports(&self) -> &[VMFunctionImport] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMFunctionImport>(
                self.vmctx_plan.vmctx_function_imports_start(),
            ),
            self.vmctx_plan.num_imported_funcs() as usize,
        )
    }

    unsafe fn vmctx_table_imports(&self) -> &[VMTableImport] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMTableImport>(self.vmctx_plan.vmctx_table_imports_start()),
            self.vmctx_plan.num_imported_tables() as usize,
        )
    }

    unsafe fn vmctx_memory_imports(&self) -> &[VMMemoryImport] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMMemoryImport>(self.vmctx_plan.vmctx_memory_imports_start()),
            self.vmctx_plan.num_imported_memories() as usize,
        )
    }

    unsafe fn vmctx_global_imports(&self) -> &[VMGlobalImport] {
        slice::from_raw_parts(
            self.vmctx_plus_offset::<VMGlobalImport>(self.vmctx_plan.vmctx_global_imports_start()),
            self.vmctx_plan.num_imported_globals() as usize,
        )
    }

    unsafe fn vmctx_plus_offset<T>(&self, offset: u32) -> *const T {
        self.vmctx
            .as_ptr()
            .byte_add(usize::try_from(offset).unwrap())
            .cast()
    }

    unsafe fn vmctx_plus_offset_mut<T>(&mut self, offset: u32) -> *mut T {
        self.vmctx
            .as_ptr()
            .byte_add(usize::try_from(offset).unwrap())
            .cast()
    }

    fn get_func_ref(&mut self, func_index: FuncIndex) -> Option<*mut VMFuncRef> {
        if func_index == FuncIndex::reserved_value() {
            return None;
        }

        let func = &self.module_info.module.functions[func_index];
        let sig = func.signature;

        log::debug!("get_func_ref {func_index:?} -> {func:?}");
        let func_ref: *mut VMFuncRef = unsafe {
            self.vmctx_plus_offset_mut::<VMFuncRef>(self.vmctx_plan.vmctx_func_ref(func.func_ref))
        };

        self.construct_func_ref(func_index, sig, func_ref);

        Some(func_ref)
    }

    fn construct_func_ref(
        &self,
        _func_index: FuncIndex,
        _sig: ModuleInternedTypeIndex,
        out: *mut VMFuncRef,
    ) {
        unsafe {
            out.write(VMFuncRef {
                vmctx: self.vmctx.as_ptr(),
            });
        }
    }

    pub fn global_ptr(&mut self, index: DefinedGlobalIndex) -> *mut VMGlobalDefinition {
        unsafe { self.vmctx_plus_offset_mut(self.vmctx_plan.vmctx_global_definition(index)) }
    }

    fn defined_or_imported_global_ptr(
        &mut self,
        global_index: GlobalIndex,
    ) -> *mut VMGlobalDefinition {
        if let Some(index) = self.module_info.module.defined_global_index(global_index) {
            self.global_ptr(index)
        } else {
            todo!()
        }
    }

    fn table_ptr(&mut self, index: DefinedTableIndex) -> *mut VMTableDefinition {
        unsafe { self.vmctx_plus_offset_mut(self.vmctx_plan.vmctx_table_definition(index)) }
    }

    fn imported_table(&self, index: TableIndex) -> &VMTableImport {
        unsafe { &*self.vmctx_plus_offset(self.vmctx_plan.vmctx_table_import(index)) }
    }

    pub unsafe fn table_index(&mut self, table: &VMTableDefinition) -> DefinedTableIndex {
        let index = DefinedTableIndex::new(
            usize::try_from(
                core::ptr::from_ref(table).offset_from(self.table_ptr(DefinedTableIndex::new(0))),
            )
            .unwrap(),
        );
        assert!(index.index() < self.tables.len());
        index
    }

    pub fn table_init_segment(
        &mut self,
        const_eval: &mut ConstExprEvaluator,
        def_table_index: DefinedTableIndex,
        elements: &TableSegmentElements,
        dst: u32,
        src: u32,
        len: u32,
    ) -> Result<(), Trap> {
        match elements {
            TableSegmentElements::Functions(funcs) => {
                let elements = funcs
                    .get(src as usize..)
                    .and_then(|s| s.get(..len as usize))
                    .ok_or(Trap::TableOutOfBounds)?;

                // annoying allocation here for lifetime reasons
                let elements: Vec<_> = elements
                    .iter()
                    .map(|idx| {
                        log::debug!("table segment elements: func_index {idx:?}");
                        self.get_func_ref(*idx).unwrap_or(ptr::null_mut())
                    })
                    .collect();

                let table = &mut self.tables[def_table_index];
                table.init_func(dst, elements.into_iter())?;
            }
            TableSegmentElements::Expressions(exprs) => {
                let exprs = exprs
                    .get(src as usize..)
                    .and_then(|s| s.get(..len as usize))
                    .ok_or(Trap::TableOutOfBounds)?;

                let table_index = self.module_info.module.table_index(def_table_index);
                match self.module_info.module.table_plans[table_index]
                    .table
                    .wasm_ty
                    .heap_type
                {
                    WasmHeapType::Func | WasmHeapType::ConcreteFunc(_) | WasmHeapType::NoFunc => {
                        let exprs: Vec<_> = exprs
                            .iter()
                            .map(|expr| unsafe { const_eval.eval(self, expr).funcref.cast() })
                            .collect();

                        self.tables[def_table_index].init_func(dst, exprs.into_iter())?;
                    }
                    WasmHeapType::Extern
                    | WasmHeapType::NoExtern
                    | WasmHeapType::Eq
                    | WasmHeapType::Array
                    | WasmHeapType::ConcreteArray(_)
                    | WasmHeapType::Struct
                    | WasmHeapType::ConcreteStruct(_)
                    | WasmHeapType::Any
                    | WasmHeapType::I31
                    | WasmHeapType::None => todo!(),
                }
            }
        }

        Ok(())
    }

    fn defined_or_imported_memory(&self, index: MemoryIndex) -> &VMMemoryDefinition {
        unsafe {
            if let Some(def_index) = self.module_info.module.defined_memory_index(index) {
                &self.vmctx_memory_definitions()[def_index.index()]

                // *self.vmctx_plus_offset::<*mut VMMemoryDefinition>(
                //     self.vmctx_plan.vmctx_memory_pointer(def_index),
                // )
            } else {
                let import = &*self.vmctx_plus_offset::<VMMemoryImport>(
                    self.vmctx_plan.vmctx_memory_import(index),
                );
                &*import.from
            }
        }
    }
}

fn initialize_vmctx(
    const_eval: &mut ConstExprEvaluator,
    store: &Store,
    instance: Instance,
    module: &TranslatedModule,
) -> Result<(), Trap> {
    let mut data = store.instance_data_mut(instance);

    unsafe {
        // initialize vmctx magic number
        let offset = data.vmctx_plan.vmctx_magic();
        *data.vmctx_plus_offset_mut(offset) = VMCONTEXT_MAGIC;

        // initialize defined globals
        for (global_index, expr) in &module.global_initializers {
            let val = const_eval.eval(&mut data, expr);

            log::debug!("global initializer {global_index:?} {:?}", val.v128);

            let offset = data.vmctx_plan.vmctx_global_definition(global_index);
            let ptr: *mut VMGlobalDefinition = data.vmctx_plus_offset_mut(offset);

            ptr.write(VMGlobalDefinition::from_vmval(val));
        }

        // initialize defined tables
        let tables = mem::take(&mut data.tables);
        for (table_index, table) in &tables {
            let offset = data.vmctx_plan.vmctx_table_definition(table_index);
            let ptr: *mut VMTableDefinition = data.vmctx_plus_offset_mut(offset);

            ptr.write(table.as_vmtable());
        }
        data.tables = tables;

        // initialize defined and owned memories
        let mut memories = mem::take(&mut data.memories);
        for (memory_index, memory) in &mut memories {
            let owned_memory_index = module.owned_memory_index(memory_index);
            let offset = data.vmctx_plan.vmctx_memory_definition(owned_memory_index);
            let ptr = data.vmctx_plus_offset_mut::<VMMemoryDefinition>(offset);
            ptr.write(memory.as_vmmemory());

            let offset = data.vmctx_plan.vmctx_memory_pointer(memory_index);
            let ptr_ptr = data.vmctx_plus_offset_mut::<*mut VMMemoryDefinition>(offset);
            ptr_ptr.write(ptr);
        }
        data.memories = memories;

        // TODO initialize imported functions
        // TODO initialize imported tables
        // TODO initialize imported memories
        // TODO initialize imported globals
    }

    Ok(())
}

fn initialize_tables(
    const_eval: &mut ConstExprEvaluator,
    store: &Store,
    instance: Instance,
    module: &TranslatedModule,
) -> Result<(), Trap> {
    for (def_table_index, initial_value) in &module.table_initializers.initial_values {
        match initial_value {
            TableInitialValue::RefNull => {}
            TableInitialValue::ConstExpr(expr) => {
                let mut data = store.instance_data_mut(instance);
                let val = const_eval.eval(&mut data, expr);

                let table_index = module.table_index(def_table_index);
                let table = &mut data.tables[def_table_index];
                match module.table_plans[table_index].table.wasm_ty.heap_type {
                    WasmHeapType::Func | WasmHeapType::ConcreteFunc(_) | WasmHeapType::NoFunc => {
                        let funcref = unsafe { val.funcref.cast::<VMFuncRef>() };
                        let items = (0..table.len()).map(|_| funcref);
                        table.init_func(0, items)?;
                    }

                    WasmHeapType::Extern
                    | WasmHeapType::NoExtern
                    | WasmHeapType::Eq
                    | WasmHeapType::Array
                    | WasmHeapType::ConcreteArray(_)
                    | WasmHeapType::Struct
                    | WasmHeapType::ConcreteStruct(_)
                    | WasmHeapType::Any
                    | WasmHeapType::I31
                    | WasmHeapType::None => todo!(),
                }
            }
        }
    }

    for segment in &module.table_initializers.segments {
        log::debug!("initializing from segment {segment:?}");

        let start = if let Some(base) = segment.base {
            let mut data = store.instance_data_mut(instance);
            let base = unsafe { *(*data.defined_or_imported_global_ptr(base)).as_u32() };

            segment
                .offset
                .checked_add(base)
                .expect("element segment global base overflows")
        } else {
            segment.offset
        };

        with_defined_table_index_and_instance(
            store,
            instance,
            segment.table_index,
            |def_table_index, data| {
                data.table_init_segment(
                    const_eval,
                    def_table_index,
                    &segment.elements,
                    start,
                    0,
                    u32::try_from(segment.elements.len()).unwrap(),
                )
            },
        )?;
    }

    Ok(())
}

fn initialize_memories(
    store: &Store,
    instance: Instance,
    module: &TranslatedModule,
) -> Result<(), Trap> {
    for init in &module.memory_initializers.runtime {
        let def_index = module.defined_memory_index(init.memory_index).unwrap();
        let mut data = store.instance_data_mut(instance);

        data.memories[def_index].inner.extend_from_slice(init.bytes);
    }

    Ok(())
}

fn with_defined_table_index_and_instance<R>(
    store: &Store,
    instance: Instance,
    index: TableIndex,
    f: impl FnOnce(DefinedTableIndex, &mut InstanceData) -> R,
) -> R {
    let def_table_index = store
        .instance_data(instance)
        .module_info
        .module
        .defined_table_index(index);

    if let Some(def_table_index) = def_table_index {
        let mut data = store.instance_data_mut(instance);

        f(def_table_index, &mut data)
    } else {
        let (foreign_instance, foreign_table_def) = {
            let data = store.instance_data(instance);
            let import = data.imported_table(index);
            let foreign_instance = store.instance_for_vmctx(import.vmctx);
            let foreign_table_def = import.from;

            (foreign_instance, foreign_table_def)
        };

        let mut foreign_instance = store.instance_data_mut(foreign_instance);
        let foreign_table_index = unsafe { foreign_instance.table_index(&*foreign_table_def) };
        f(foreign_table_index, &mut foreign_instance)
    }
}
