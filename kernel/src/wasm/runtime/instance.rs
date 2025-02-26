#![expect(
    clippy::undocumented_unsafe_blocks,
    reason = "too many trivial unsafe blocks"
)]

use crate::arch;
use crate::vm::AddressSpace;
use crate::wasm::indices::{
    DataIndex, DefinedGlobalIndex, DefinedMemoryIndex, DefinedTableIndex, ElemIndex, EntityIndex,
    FuncIndex, GlobalIndex, MemoryIndex, TableIndex, VMSharedTypeIndex,
};
use crate::wasm::runtime::builtins::VMBuiltinFunctionsArray;
use crate::wasm::runtime::memory::Memory;
use crate::wasm::runtime::table::Table;
use crate::wasm::runtime::vmcontext::{
    VMArrayCallFunction, VMGlobalDefinition, VMWasmCallFunction,
};
use crate::wasm::runtime::{
    ConstExprEvaluator, Export, ExportedFunction, ExportedGlobal, ExportedMemory, ExportedTable,
    Imports, InstanceAllocator, OwnedVMContext, VMContext, VMFuncRef, VMFunctionImport,
    VMGlobalImport, VMMemoryDefinition, VMMemoryImport, VMOffsets, VMOpaqueContext,
    VMTableDefinition, VMTableImport, VMCONTEXT_MAGIC,
};
use crate::wasm::translate::{TableInitialValue, TableSegmentElements};
use crate::wasm::{Extern, Module, Store};
use alloc::vec;
use alloc::vec::Vec;
use core::ptr::NonNull;
use core::range::Range;
use core::{fmt, mem, ptr, slice};
use cranelift_entity::packed_option::ReservedValue;
use cranelift_entity::{EntityRef, EntitySet, PrimaryMap};

#[derive(Debug)]
pub struct Instance {
    module: Module,

    vmctx: OwnedVMContext,
    tables: PrimaryMap<DefinedTableIndex, Table>,
    memories: PrimaryMap<DefinedMemoryIndex, Memory>,
    dropped_elems: EntitySet<ElemIndex>,
    dropped_data: EntitySet<DataIndex>,

    pub(crate) exports: Vec<Option<Extern>>,
}

impl Instance {
    pub unsafe fn new_unchecked(
        store: &mut Store,
        const_eval: &mut ConstExprEvaluator,
        module: Module,
        imports: Imports,
    ) -> crate::wasm::Result<Self> {
        let (mut vmctx, mut tables, mut memories) = store.alloc.allocate_module(&module)?;

        tracing::trace!("initializing instance");
        unsafe {
            initialize_vmctx(
                const_eval,
                &mut vmctx,
                &mut tables,
                &mut memories,
                &module,
                imports,
            )?;
            initialize_tables(const_eval, &mut tables, &module)?;

            let mut aspace = store.alloc.0.lock();
            initialize_memories(&mut aspace, const_eval, &mut memories, &module)?;
        }
        tracing::trace!("done initializing instance");

        let exports = vec![None; module.exports().len()];

        Ok(Self {
            vmctx,
            tables,
            memories,
            dropped_elems: module.translated().active_table_initializers.clone(),
            dropped_data: module.translated().active_memory_initializers.clone(),
            exports,
            module,
        })
    }

    pub fn module(&self) -> &Module {
        &self.module
    }

    pub fn vmctx(&self) -> *const VMContext {
        self.vmctx.as_ptr()
    }

    pub fn vmctx_mut(&mut self) -> *mut VMContext {
        self.vmctx.as_mut_ptr()
    }

    pub fn get_exported_func(&mut self, index: FuncIndex) -> ExportedFunction {
        let func_ref = self.get_func_ref(index).unwrap();
        let func_ref = NonNull::new(func_ref).unwrap();
        ExportedFunction { func_ref }
    }

    fn get_func_ref(&mut self, index: FuncIndex) -> Option<*mut VMFuncRef> {
        if index == FuncIndex::reserved_value() {
            return None;
        }

        let func = &self.module().translated().functions[index];

        // Safety: offsets are small so no overflow *should* happen. TODO ensure this
        unsafe {
            let func_ref: *mut VMFuncRef = self.vmctx.plus_offset_mut::<VMFuncRef>(
                self.module().offsets().vmctx_vmfunc_ref(func.func_ref),
            );
            Some(func_ref)
        }
    }
    pub fn imported_function(&self, index: FuncIndex) -> &VMFunctionImport {
        // Safety: offsets are small so no overflow *should* happen. TODO ensure this
        unsafe {
            &*self
                .vmctx
                .plus_offset(self.module().offsets().vmctx_vmfunction_import(index))
        }
    }

    pub fn get_exported_table(&mut self, index: TableIndex) -> ExportedTable {
        let (definition, vmctx) =
            if let Some(def_index) = self.module().translated().defined_table_index(index) {
                (self.table_ptr(def_index), self.vmctx.as_mut_ptr())
            } else {
                let import = self.imported_table(index);
                (import.from, import.vmctx)
            };

        ExportedTable {
            definition,
            vmctx,
            table: self.module().translated().tables[index].clone(),
        }
    }
    pub fn table_ptr(&mut self, index: DefinedTableIndex) -> *mut VMTableDefinition {
        // Safety: offsets are small so no overflow *should* happen. TODO ensure this
        unsafe {
            self.vmctx
                .plus_offset_mut(self.module().offsets().vmctx_vmtable_definition(index))
        }
    }
    pub fn imported_table(&self, index: TableIndex) -> &VMTableImport {
        // Safety: offsets are small so no overflow *should* happen. TODO ensure this
        unsafe {
            &*self
                .vmctx
                .plus_offset(self.module().offsets().vmctx_vmtable_import(index))
        }
    }

    pub fn get_exported_memory(&mut self, index: MemoryIndex) -> ExportedMemory {
        let (definition, vmctx) =
            if let Some(def_index) = self.module().translated().defined_memory_index(index) {
                (self.memory_ptr(def_index), self.vmctx.as_mut_ptr())
            } else {
                let import = self.imported_memory(index);
                (import.from, import.vmctx)
            };

        ExportedMemory {
            definition,
            vmctx,
            memory: self.module().translated().memories[index].clone(),
        }
    }
    pub fn memory_ptr(&mut self, index: DefinedMemoryIndex) -> *mut VMMemoryDefinition {
        // Safety: offsets are small so no overflow *should* happen. TODO ensure this
        unsafe {
            self.vmctx
                .plus_offset_mut(self.module().offsets().vmctx_vmmemory_definition(index))
        }
    }
    pub fn imported_memory(&self, index: MemoryIndex) -> &VMMemoryImport {
        // Safety: offsets are small so no overflow *should* happen. TODO ensure this
        unsafe {
            &*self
                .vmctx
                .plus_offset(self.module().offsets().vmctx_vmmemory_import(index))
        }
    }

    pub fn get_exported_global(&mut self, index: GlobalIndex) -> ExportedGlobal {
        let (definition, vmctx) =
            if let Some(def_index) = self.module().translated().defined_global_index(index) {
                (self.global_ptr(def_index), self.vmctx.as_mut_ptr())
            } else {
                let import = self.imported_global(index);
                (import.from, import.vmctx)
            };

        ExportedGlobal {
            definition,
            vmctx,
            ty: self.module().translated().globals[index].clone(),
        }
    }
    pub fn global_ptr(&mut self, index: DefinedGlobalIndex) -> *mut VMGlobalDefinition {
        // Safety: offsets are small so no overflow *should* happen. TODO ensure this
        unsafe {
            self.vmctx
                .plus_offset_mut(self.module().offsets().vmctx_vmglobal_definition(index))
        }
    }
    pub fn imported_global(&self, index: GlobalIndex) -> &VMGlobalImport {
        // Safety: offsets are small so no overflow *should* happen. TODO ensure this
        unsafe {
            &*self
                .vmctx
                .plus_offset(self.module().offsets().vmctx_vmglobal_import(index))
        }
    }

    pub fn get_export_by_index(&mut self, index: EntityIndex) -> Export {
        match index {
            EntityIndex::Function(i) => Export::Function(self.get_exported_func(i)),
            EntityIndex::Global(i) => Export::Global(self.get_exported_global(i)),
            EntityIndex::Table(i) => Export::Table(self.get_exported_table(i)),
            EntityIndex::Memory(i) => Export::Memory(self.get_exported_memory(i)),
            EntityIndex::Tag(_) => todo!(),
        }
    }

    pub fn debug_vmctx(&self) {
        struct Dbg<'a> {
            data: &'a Instance,
        }
        impl fmt::Debug for Dbg<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                // Safety: Reading from JIT-owned memory is inherently unsafe.
                unsafe {
                    f.debug_struct("VMContext")
                        .field("magic", &self.data.vmctx_magic())
                        .field("builtin_functions", &self.data.vmctx_builtin_functions())
                        .field("type_ids", &self.data.vmctx_type_ids())
                        .field("stack_limit", &(self.data.vmctx_stack_limit() as *const u8))
                        .field(
                            "last_wasm_exit_fp",
                            &(self.data.vmctx_last_wasm_exit_fp() as *const u8),
                        )
                        .field(
                            "last_wasm_exit_pc",
                            &(self.data.vmctx_last_wasm_exit_pc() as *const u8),
                        )
                        .field(
                            "last_wasm_entry_fp",
                            &(self.data.vmctx_last_wasm_entry_fp() as *const u8),
                        )
                        .field("func_refs", &self.data.vmctx_func_refs())
                        .field("imported_functions", &self.data.vmctx_function_imports())
                        .field("imported_tables", &self.data.vmctx_table_imports())
                        .field("imported_memories", &self.data.vmctx_memory_imports())
                        .field("imported_globals", &self.data.vmctx_global_imports())
                        .field("tables", &self.data.vmctx_table_definitions())
                        .field("memories", &self.data.vmctx_memory_definitions())
                        .field("globals", &self.data.vmctx_global_definitions())
                        .finish()
                }
            }
        }

        tracing::debug!("{:#?}", Dbg { data: self });
    }

    pub(crate) unsafe fn vmctx_magic(&self) -> u32 {
        unsafe {
            *self
                .vmctx
                .plus_offset::<u32>(u32::from(self.module.offsets().static_.vmctx_magic()))
        }
    }
    pub(crate) unsafe fn vmctx_type_ids(&self) -> &[VMSharedTypeIndex] {
        unsafe {
            let ptr = *self
                .vmctx
                .plus_offset(u32::from(self.module.offsets().static_.vmctx_type_ids()));

            let len = self.module.type_collection().type_map().len();

            slice::from_raw_parts(ptr, len)
        }
    }
    pub(crate) unsafe fn vmctx_builtin_functions(&self) -> *const VMBuiltinFunctionsArray {
        unsafe {
            self.vmctx.plus_offset::<VMBuiltinFunctionsArray>(u32::from(
                self.module.offsets().static_.vmctx_builtin_functions(),
            ))
        }
    }
    pub(crate) unsafe fn vmctx_stack_limit(&self) -> usize {
        unsafe {
            *self
                .vmctx
                .plus_offset::<usize>(u32::from(self.module.offsets().static_.vmctx_stack_limit()))
        }
    }
    pub(crate) unsafe fn vmctx_last_wasm_exit_fp(&self) -> usize {
        unsafe {
            *self.vmctx.plus_offset::<usize>(u32::from(
                self.module.offsets().static_.vmctx_last_wasm_exit_fp(),
            ))
        }
    }
    pub(crate) unsafe fn vmctx_last_wasm_exit_pc(&self) -> usize {
        unsafe {
            *self.vmctx.plus_offset::<usize>(u32::from(
                self.module.offsets().static_.vmctx_last_wasm_exit_pc(),
            ))
        }
    }
    pub(crate) unsafe fn vmctx_last_wasm_entry_fp(&self) -> usize {
        unsafe {
            *self.vmctx.plus_offset::<usize>(u32::from(
                self.module.offsets().static_.vmctx_last_wasm_entry_fp(),
            ))
        }
    }
    pub(crate) unsafe fn vmctx_table_definitions(&self) -> &[VMTableDefinition] {
        unsafe {
            slice::from_raw_parts(
                self.vmctx
                    .plus_offset::<VMTableDefinition>(self.module.offsets().vmctx_tables_begin()),
                usize::try_from(self.module.translated().num_defined_tables()).unwrap(),
            )
        }
    }
    pub(crate) unsafe fn vmctx_memory_definitions(&self) -> &[VMMemoryDefinition] {
        unsafe {
            slice::from_raw_parts(
                self.vmctx.plus_offset::<VMMemoryDefinition>(
                    self.module.offsets().vmctx_memories_begin(),
                ),
                usize::try_from(self.module.translated().num_defined_memories()).unwrap(),
            )
        }
    }
    pub(crate) unsafe fn vmctx_global_definitions(&self) -> &[VMGlobalDefinition] {
        unsafe {
            slice::from_raw_parts(
                self.vmctx
                    .plus_offset::<VMGlobalDefinition>(self.module.offsets().vmctx_globals_begin()),
                usize::try_from(self.module.translated().num_defined_globals()).unwrap(),
            )
        }
    }
    pub(crate) unsafe fn vmctx_func_refs(&self) -> &[VMFuncRef] {
        unsafe {
            slice::from_raw_parts(
                self.vmctx
                    .plus_offset::<VMFuncRef>(self.module.offsets().vmctx_func_refs_begin()),
                usize::try_from(self.module.translated().num_escaped_funcs()).unwrap(),
            )
        }
    }
    pub(crate) unsafe fn vmctx_function_imports(&self) -> &[VMFunctionImport] {
        unsafe {
            slice::from_raw_parts(
                self.vmctx.plus_offset::<VMFunctionImport>(
                    self.module.offsets().vmctx_imported_functions_begin(),
                ),
                usize::try_from(self.module.translated().num_imported_functions()).unwrap(),
            )
        }
    }
    pub(crate) unsafe fn vmctx_table_imports(&self) -> &[VMTableImport] {
        unsafe {
            slice::from_raw_parts(
                self.vmctx.plus_offset::<VMTableImport>(
                    self.module.offsets().vmctx_imported_tables_begin(),
                ),
                usize::try_from(self.module.translated().num_imported_tables()).unwrap(),
            )
        }
    }
    pub(crate) unsafe fn vmctx_memory_imports(&self) -> &[VMMemoryImport] {
        unsafe {
            slice::from_raw_parts(
                self.vmctx.plus_offset::<VMMemoryImport>(
                    self.module.offsets().vmctx_imported_memories_begin(),
                ),
                usize::try_from(self.module.translated().num_imported_memories()).unwrap(),
            )
        }
    }
    pub(crate) unsafe fn vmctx_global_imports(&self) -> &[VMGlobalImport] {
        unsafe {
            slice::from_raw_parts(
                self.vmctx.plus_offset::<VMGlobalImport>(
                    self.module.offsets().vmctx_imported_globals_begin(),
                ),
                usize::try_from(self.module.translated().num_imported_globals()).unwrap(),
            )
        }
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "imports should be a linear type"
)]
#[expect(clippy::unnecessary_wraps, reason = "TODO")]
unsafe fn initialize_vmctx(
    const_eval: &mut ConstExprEvaluator,
    vmctx: &mut OwnedVMContext,
    tables: &mut PrimaryMap<DefinedTableIndex, Table>,
    memories: &mut PrimaryMap<DefinedMemoryIndex, Memory>,
    module: &Module,
    imports: Imports,
) -> crate::wasm::Result<()> {
    unsafe {
        let offsets = module.offsets();

        // initialize vmctx magic
        tracing::trace!("initializing vmctx magic");
        *vmctx.plus_offset_mut(u32::from(offsets.static_.vmctx_magic())) = VMCONTEXT_MAGIC;

        // Initialize the built-in functions
        tracing::trace!("initializing built-in functions array ptr");
        *vmctx.plus_offset_mut::<*const VMBuiltinFunctionsArray>(u32::from(
            offsets.static_.vmctx_builtin_functions(),
        )) = ptr::from_ref(&VMBuiltinFunctionsArray::INIT);

        // initialize the type ids array ptr
        tracing::trace!("initializing type ids array ptr");
        let type_ids = module.type_ids();
        *vmctx.plus_offset_mut(u32::from(offsets.static_.vmctx_type_ids())) = type_ids.as_ptr();

        // initialize func_refs array
        tracing::trace!("initializing func refs array");
        initialize_vmfunc_refs(vmctx, &module, &imports, offsets);

        // initialize the imports
        tracing::trace!("initializing function imports");
        ptr::copy_nonoverlapping(
            imports.functions.as_ptr(),
            vmctx.plus_offset_mut::<VMFunctionImport>(offsets.vmctx_imported_functions_begin()),
            imports.functions.len(),
        );
        tracing::trace!("initialized table imports");
        ptr::copy_nonoverlapping(
            imports.tables.as_ptr(),
            vmctx.plus_offset_mut::<VMTableImport>(offsets.vmctx_imported_tables_begin()),
            imports.tables.len(),
        );
        tracing::trace!("initialized memory imports");
        ptr::copy_nonoverlapping(
            imports.memories.as_ptr(),
            vmctx.plus_offset_mut::<VMMemoryImport>(offsets.vmctx_imported_memories_begin()),
            imports.memories.len(),
        );
        tracing::trace!("initialized global imports");
        ptr::copy_nonoverlapping(
            imports.globals.as_ptr(),
            vmctx.plus_offset_mut::<VMGlobalImport>(offsets.vmctx_imported_globals_begin()),
            imports.globals.len(),
        );

        // Initialize the defined tables
        tracing::trace!("initializing defined tables");
        for def_index in module
            .translated()
            .tables
            .keys()
            .filter_map(|index| module.translated().defined_table_index(index))
        {
            let ptr = vmctx
                .plus_offset_mut::<VMTableDefinition>(offsets.vmctx_vmtable_definition(def_index));
            ptr.write(tables[def_index].as_vmtable_definition());
        }

        // Initialize the `defined_memories` table.
        tracing::trace!("initializing defined memories");
        for (def_index, plan) in module
            .translated()
            .memories
            .iter()
            .filter_map(|(index, plan)| {
                Some((module.translated().defined_memory_index(index)?, plan))
            })
        {
            assert!(!plan.shared, "shared memories are not currently supported");

            let ptr = vmctx.plus_offset_mut::<VMMemoryDefinition>(
                offsets.vmctx_vmmemory_definition(def_index),
            );

            ptr.write(memories[def_index].as_vmmemory_definition());
        }

        // Initialize the `defined_globals` table.
        tracing::trace!("initializing defined globals");
        for (def_index, init_expr) in &module.translated().global_initializers {
            let val = const_eval.eval(init_expr);
            let ptr = vmctx.plus_offset_mut::<VMGlobalDefinition>(
                module.offsets().vmctx_vmglobal_definition(def_index),
            );
            ptr.write(VMGlobalDefinition::from_vmval(val));
        }

        Ok(())
    }
}

fn initialize_vmfunc_refs(
    vmctx: &mut OwnedVMContext,
    module: &&Module,
    imports: &Imports,
    offsets: &VMOffsets,
) {
    for (index, func) in module
        .translated()
        .functions
        .iter()
        .filter(|(_, f)| f.is_escaping())
    {
        let func_ref = if let Some(def_index) = module.translated().defined_func_index(index) {
            let info = &module.function_info()[def_index];
            let array_call = info
                .host_to_wasm_trampoline
                .expect("escaping function requires trampoline");
            let wasm_call = info.wasm_func_loc;

            VMFuncRef {
                array_call: unsafe {
                    mem::transmute::<usize, VMArrayCallFunction>(
                        module.code().resolve_function_loc(array_call),
                    )
                },
                wasm_call: NonNull::new(
                    module.code().resolve_function_loc(wasm_call) as *mut VMWasmCallFunction
                )
                .unwrap(),
                vmctx: VMOpaqueContext::from_vmcontext(vmctx.as_mut_ptr()),
                type_index: {
                    let index = module.translated().types[func.signature];
                    module.type_collection().lookup_shared_type(index).unwrap()
                },
            }
        } else {
            let import = &imports.functions[index.index()];
            let type_index = module.translated().types[func.signature];
            let type_index = module
                .type_collection()
                .lookup_shared_type(type_index)
                .unwrap();
            VMFuncRef {
                array_call: import.array_call,
                wasm_call: import.wasm_call,
                vmctx: import.vmctx,
                type_index,
            }
        };

        let into =
            unsafe { vmctx.plus_offset_mut::<VMFuncRef>(offsets.vmctx_vmfunc_ref(func.func_ref)) };

        // Safety: we have a `&mut self`, so we have exclusive access
        // to this Instance.
        unsafe {
            ptr::write(into, func_ref);
        }
    }
}

unsafe fn initialize_tables(
    const_eval: &mut ConstExprEvaluator,
    tables: &mut PrimaryMap<DefinedTableIndex, Table>,
    module: &Module,
) -> crate::wasm::Result<()> {
    // update initial values
    for (def_index, init) in &module.translated().table_initializers.initial_values {
        let val = match init {
            TableInitialValue::RefNull => None,
            TableInitialValue::ConstExpr(expr) => {
                let funcref = const_eval.eval(expr).get_funcref();
                // TODO assert funcref ptr is valid
                Some(NonNull::new(funcref.cast()).unwrap())
            }
        };

        tables[def_index].elements_mut().fill(val);
    }

    // run active elements
    for segment in &module.translated().table_initializers.segments {
        let elements: Vec<_> = match &segment.elements {
            TableSegmentElements::Functions(_funcs) => {
                todo!()
            }
            TableSegmentElements::Expressions(exprs) => exprs
                .iter()
                .map(|expr| -> crate::wasm::Result<Option<NonNull<VMFuncRef>>> {
                    let funcref = const_eval.eval(expr).get_funcref();
                    // TODO assert funcref ptr is valid
                    Ok(Some(NonNull::new(funcref.cast()).unwrap()))
                })
                .collect::<Result<Vec<_>, _>>()?,
        };

        let offset = const_eval.eval(&segment.offset);
        let offset = usize::try_from(offset.get_u64()).unwrap();

        if let Some(def_index) = module.translated().defined_table_index(segment.table_index) {
            tables[def_index].elements_mut()[offset..offset + elements.len()]
                .copy_from_slice(&elements);
        } else {
            todo!("initializing imported table")
        }
    }

    Ok(())
}

#[expect(clippy::unnecessary_wraps, reason = "TODO")]
unsafe fn initialize_memories(
    aspace: &mut AddressSpace,
    const_eval: &mut ConstExprEvaluator,
    memories: &mut PrimaryMap<DefinedMemoryIndex, Memory>,
    module: &Module,
) -> crate::wasm::Result<()> {
    for init in &module.translated().memory_initializers {
        let offset = const_eval.eval(&init.offset);
        let offset = usize::try_from(offset.get_u64()).unwrap();

        if let Some(def_index) = module.translated().defined_memory_index(init.memory_index) {
            memories[def_index].with_user_slice_mut(
                aspace,
                Range::from(offset..offset + init.data.len()),
                |slice| {
                    slice.copy_from_slice(&init.data);
                },
            );
        } else {
            todo!("initializing imported table")
        }
    }

    Ok(())
}
