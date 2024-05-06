use crate::runtime::compile::CompiledModuleInfo;
use crate::runtime::const_expr::ConstExprEvaluator;
use crate::runtime::instantiate::export::ExportFunction;
use crate::runtime::instantiate::stack::Stack;
use crate::runtime::instantiate::CodeMemory;
use crate::runtime::{VMContext, VMContextOffsets, VMFuncRef, VMGlobalDefinition, VMCONTEXT_MAGIC};
use alloc::sync::Arc;
use core::ptr::NonNull;
use core::{mem, ptr};
use cranelift_codegen::entity::entity_impl;
use cranelift_wasm::{DefinedFuncIndex, DefinedGlobalIndex, FuncIndex};

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct InstanceHandle(u32);
entity_impl!(InstanceHandle);

#[derive(Debug)]
pub struct InstanceData<'wasm> {
    pub module_info: CompiledModuleInfo<'wasm>,
    pub code: Arc<CodeMemory>,
    pub stack: Stack,
    pub vmctx: NonNull<VMContext>,
    pub vmctx_offsets: VMContextOffsets,
}

impl<'wasm> InstanceData<'wasm> {
    pub fn get_exported_func(&mut self, func_index: FuncIndex) -> ExportFunction {
        if let Some(def_func_index) = self.module_info.module.defined_func_index(func_index) {
            let ptr =
                unsafe { self.vmctx_plus_offset_mut(self.vmctx_offsets.vmfunc_ref(func_index)) };

            self.make_func_ref(def_func_index, ptr);

            // Safety: `make_func_ref` ensures the pointer is initialized
            ExportFunction {
                func_ref: unsafe { NonNull::new_unchecked(ptr) },
            }
        } else {
            todo!("imported function")
        }
    }

    fn make_func_ref(&mut self, def_func_index: DefinedFuncIndex, into: *mut VMFuncRef) {
        let native_call = self.module_info.funcs[def_func_index]
            .native_to_wasm_trampoline
            .expect("should have native-to-Wasm trampoline for escaping function");

        let native_call = self.code.resolve_function_loc(native_call);

        unsafe { ptr::write(into, VMFuncRef { native_call }) }
    }

    pub unsafe fn vmctx_plus_offset_mut<T>(&mut self, offset: u32) -> *mut T {
        self.vmctx.cast::<u8>().as_ptr().add(offset as usize).cast()
    }

    pub fn global_ptr(&mut self, def_global_index: DefinedGlobalIndex) -> *mut VMGlobalDefinition {
        unsafe {
            self.vmctx_plus_offset_mut(self.vmctx_offsets.vmglobal_definition(def_global_index))
        }
    }

    pub fn initialize(&mut self) {
        unsafe {
            *self.vmctx_plus_offset_mut(self.vmctx_offsets.vmctx_magic()) = VMCONTEXT_MAGIC;
        }

        //  TODO init builtin functions array
        //  TODO init imports
        //      - copy from imports.functions
        //      - copy from imports.tables
        //      - copy from imports.memories
        //      - copy from imports.globals
        //  - dont set set stack limit, its set at call time
        //  - dont init last_wasm_exit_fp, last_wasm_exit_pc, or last_wasm_entry_sp bc zero initialization

        let mut eval_ctx = ConstExprEvaluator::default();

        self.initialize_globals(&mut eval_ctx);
        self.initialize_tables();
        self.initialize_memories();
    }

    fn initialize_globals(&mut self, eval_ctx: &mut ConstExprEvaluator) {
        let initializers = mem::take(&mut self.module_info.module.global_initializers);

        for (def_global_index, global_init) in initializers {
            let val = eval_ctx.eval(self, &global_init);

            unsafe {
                self.global_ptr(def_global_index)
                    .write(VMGlobalDefinition::from_val_raw(val));
            }
        }
    }
    fn initialize_tables(&mut self) {
        // Initialize tables from const init exprs
        //  - init tables (by using VMTableDefinition from Instance)
    }
    fn initialize_memories(&mut self) {
        // Initialize memories from const init exprs
        //  - init memories
        //      - insert VMMemoryDefinition for every not-shared, not-imported memory
        //      - insert *mut VMMemoryDefinition for every not-shared, not-imported memory
        //      - insert *mut VMMemoryDefinition for every not-imported, shared memory
    }
    fn run_start(&mut self) {
        // IF present => run start function
    }
}
