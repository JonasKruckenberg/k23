use super::TranslatedModule;
use crate::runtime::utils::{reference_type, value_type, wasm_call_signature};
use crate::runtime::vmcontext::VMContextPlan;
use crate::runtime::{NS_WASM_FUNC, WASM_PAGE_SIZE};
use alloc::vec;
use alloc::vec::Vec;
use cranelift_codegen::cursor::FuncCursor;
use cranelift_codegen::entity::PrimaryMap;
use cranelift_codegen::ir::immediates::Offset32;
use cranelift_codegen::ir::types::{I32, I64};
use cranelift_codegen::ir::{
    ExtFuncData, ExternalName, Fact, FuncRef, Function, GlobalValue, GlobalValueData, Inst,
    InstBuilder, MemFlags, MemoryType, MemoryTypeData, MemoryTypeField, SigRef, Signature, Type,
    UserExternalName, Value,
};
use cranelift_codegen::isa::{TargetFrontendConfig, TargetIsa};
use cranelift_frontend::FunctionBuilder;
use cranelift_wasm::wasmparser::UnpackedIndex;
use cranelift_wasm::{
    FuncIndex, GlobalIndex, GlobalVariable, Heap, HeapData, HeapStyle, MemoryIndex,
    ModuleInternedTypeIndex, TableIndex, TargetEnvironment, TypeConvert, TypeIndex,
    WasmHeapTopType, WasmHeapType, WasmResult, WasmSubType,
};

pub struct FunctionEnvironment<'module_env, 'wasm> {
    isa: &'module_env dyn TargetIsa,
    module: &'module_env TranslatedModule<'wasm>,
    types: &'module_env PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,

    heaps: PrimaryMap<Heap, HeapData>,

    vmctx_plan: VMContextPlan,
    vmctx: Option<GlobalValue>,
    pcc_vmctx_memtype: Option<MemoryType>,
}

impl<'module_env, 'wasm> FunctionEnvironment<'module_env, 'wasm> {
    pub fn new(
        isa: &'module_env dyn TargetIsa,
        module: &'module_env TranslatedModule<'wasm>,
        types: &'module_env PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
    ) -> Self {
        Self {
            isa,
            module,
            types,

            heaps: PrimaryMap::default(),

            vmctx_plan: VMContextPlan::for_module(isa, module),
            vmctx: None,
            pcc_vmctx_memtype: None,
        }
    }

    pub fn vmctx_stack_limit_offset(&self) -> u32 {
        self.vmctx_plan.stack_limit()
    }

    fn vmctx(&mut self, func: &mut Function) -> GlobalValue {
        self.vmctx.unwrap_or_else(|| {
            let vmctx = func.create_global_value(GlobalValueData::VMContext);
            if self.isa.flags().enable_pcc() {
                // Create a placeholder memtype for the vmctx; we'll
                // add fields to it as we lazily create HeapData
                // structs and global values.
                let vmctx_memtype = func.create_memory_type(MemoryTypeData::Struct {
                    size: 0,
                    fields: vec![],
                });

                self.pcc_vmctx_memtype = Some(vmctx_memtype);
                func.global_value_facts[vmctx] = Some(Fact::Mem {
                    ty: vmctx_memtype,
                    min_offset: 0,
                    max_offset: 0,
                    nullable: false,
                });
            }

            self.vmctx = Some(vmctx);
            vmctx
        })
    }

    fn vmctx_val(&mut self, pos: &mut FuncCursor<'_>) -> Value {
        let pointer_type = self.pointer_type();
        let vmctx = self.vmctx(pos.func);
        pos.ins().global_value(pointer_type, vmctx)
    }
}

impl<'module_env, 'wasm> TargetEnvironment for FunctionEnvironment<'module_env, 'wasm> {
    fn target_config(&self) -> TargetFrontendConfig {
        self.isa.frontend_config()
    }

    fn heap_access_spectre_mitigation(&self) -> bool {
        self.isa.flags().enable_heap_access_spectre_mitigation()
    }

    fn proof_carrying_code(&self) -> bool {
        self.isa.flags().enable_pcc()
    }

    fn reference_type(&self, wasm_ty: WasmHeapType) -> (Type, bool) {
        let ty = reference_type(wasm_ty, self.pointer_type());
        let needs_stack_map = match wasm_ty.top() {
            WasmHeapTopType::Extern | WasmHeapTopType::Any => true,
            WasmHeapTopType::Func => false,
        };
        (ty, needs_stack_map)
    }
}

impl<'module_env, 'wasm> TypeConvert for FunctionEnvironment<'module_env, 'wasm> {
    fn lookup_heap_type(&self, index: UnpackedIndex) -> WasmHeapType {
        todo!()
    }

    fn lookup_type_index(
        &self,
        index: cranelift_wasm::wasmparser::UnpackedIndex,
    ) -> cranelift_wasm::EngineOrModuleTypeIndex {
        todo!()
    }
}

impl<'module_env, 'wasm> cranelift_wasm::FuncEnvironment
    for FunctionEnvironment<'module_env, 'wasm>
{
    fn param_needs_stack_map(&self, _signature: &Signature, index: usize) -> bool {
        false
    }

    fn sig_ref_result_needs_stack_map(&self, sig_ref: SigRef, index: usize) -> bool {
        false
    }

    fn func_ref_result_needs_stack_map(
        &self,
        func: &Function,
        func_ref: FuncRef,
        index: usize,
    ) -> bool {
        false
    }

    fn make_global(
        &mut self,
        func: &mut Function,
        global_index: GlobalIndex,
    ) -> WasmResult<GlobalVariable> {
        let vmctx = self.vmctx(func);
        let ty = self.module.globals[global_index].wasm_ty;

        if ty.is_vmgcref_type() {
            // Although reference-typed globals live at the same memory location as
            // any other type of global at the same index would, getting or
            // setting them requires ref counting barriers. Therefore, we need
            // to use `GlobalVariable::Custom`, as that is the only kind of
            // `GlobalVariable` for which `cranelift-wasm` supports custom
            // access translation.
            return Ok(GlobalVariable::Custom);
        }

        let offset = if let Some(def_index) = self.module.defined_global_index(global_index) {
            i32::try_from(self.vmctx_plan.vmctx_global_definition(def_index)).unwrap()
        } else {
            todo!("imported memories")
        };

        Ok(GlobalVariable::Memory {
            gv: vmctx,
            offset: offset.into(),
            ty: value_type(self.isa, ty),
        })
    }

    fn heaps(&self) -> &PrimaryMap<Heap, HeapData> {
        &self.heaps
    }

    fn make_heap(&mut self, func: &mut Function, memory_index: MemoryIndex) -> WasmResult<Heap> {
        let min_size = self.module.memory_plans[memory_index]
            .memory
            .minimum
            .checked_mul(u64::from(WASM_PAGE_SIZE))
            .unwrap_or_else(|| {
                // The only valid Wasm memory size that won't fit in a 64-bit
                // integer is the maximum memory64 size (2^64) which is one
                // larger than `u64::MAX` (2^64 - 1). In this case, just say the
                // minimum heap size is `u64::MAX`.
                debug_assert_eq!(
                    self.module.memory_plans[memory_index].memory.minimum,
                    1 << 48
                );
                u64::MAX
            });

        let max_size = self.module.memory_plans[memory_index]
            .memory
            .maximum
            .and_then(|max| max.checked_mul(u64::from(WASM_PAGE_SIZE)));

        let bound_bytes = 0x1_0000 * u64::from(WASM_PAGE_SIZE);

        let def_index = self
            .module
            .defined_memory_index(memory_index)
            .expect("imported memories");
        let owned_index = self.module.owned_memory_index(def_index);
        let base_offset =
            i32::try_from(self.vmctx_plan.vmctx_memory_definition_base(owned_index)).unwrap();

        let (base_fact, data_memtype) = if let Some(ptr_memtype) = self.pcc_vmctx_memtype {
            // Create a memtype representing the untyped memory region.
            let data_mt = func.create_memory_type(MemoryTypeData::Memory { size: bound_bytes });
            // This fact applies to any pointer to the start of the memory.
            let base_fact = Fact::Mem {
                ty: data_mt,
                min_offset: 0,
                max_offset: 0,
                nullable: false,
            };
            // Create a field in the vmctx for the base pointer.
            match &mut func.memory_types[ptr_memtype] {
                MemoryTypeData::Struct { size, fields } => {
                    let offset = u64::try_from(base_offset).unwrap();
                    fields.push(MemoryTypeField {
                        offset,
                        ty: self.isa.pointer_type(),
                        // Read-only field from the PoV of PCC checks:
                        // don't allow stores to this field. (Even if
                        // it is a dynamic memory whose base can
                        // change, that update happens inside the
                        // runtime, not in generated code.)
                        readonly: true,
                        fact: Some(base_fact.clone()),
                    });
                    *size =
                        core::cmp::max(*size, offset + u64::from(self.isa.pointer_type().bytes()));
                }
                _ => {
                    panic!("Bad memtype");
                }
            }
            // Apply a fact to the base pointer.
            (Some(base_fact), Some(data_mt))
        } else {
            (None, None)
        };

        let vmctx = self.vmctx(func);

        let mut flags = MemFlags::trusted().with_checked();
        flags.set_readonly();
        let base = func.create_global_value(GlobalValueData::Load {
            base: vmctx,
            offset: Offset32::new(base_offset),
            global_type: self.pointer_type(),
            flags,
        });
        func.global_value_facts[base] = base_fact;

        let index_type = if self.module.memory_plans[memory_index].memory.memory64 {
            I64
        } else {
            I32
        };

        Ok(self.heaps.push(HeapData {
            base,
            min_size,
            max_size,
            offset_guard_size: 0,
            style: HeapStyle::Static { bound: bound_bytes },
            index_type,
            memory_type: data_memtype,
            page_size_log2: self.module.memory_plans[memory_index].memory.page_size_log2,
        }))
    }

    fn make_indirect_sig(&mut self, func: &mut Function, index: TypeIndex) -> WasmResult<SigRef> {
        todo!()
    }

    fn make_direct_func(&mut self, func: &mut Function, index: FuncIndex) -> WasmResult<FuncRef> {
        let sig = self.module.functions[index].signature;
        let sig = &self.types[sig];
        let sig = wasm_call_signature(self.isa, sig.unwrap_func());

        let sigref = func.import_signature(sig);
        let nameref = func.declare_imported_user_function(UserExternalName {
            namespace: NS_WASM_FUNC,
            index: index.as_u32(),
        });
        let funcref = func.import_function(ExtFuncData {
            name: ExternalName::User(nameref),
            signature: sigref,
            colocated: self.module.defined_function_index(index).is_some(),
        });

        Ok(funcref)
    }

    fn translate_call(
        &mut self,
        builder: &mut FunctionBuilder,
        callee_index: FuncIndex,
        callee: FuncRef,
        call_args: &[Value],
    ) -> WasmResult<Inst> {
        let mut real_call_args = Vec::with_capacity(call_args.len() + 1);
        let vmctx = self.vmctx_val(&mut builder.cursor());

        // Handle direct calls to locally-defined functions.
        if !self.module.is_imported_function(callee_index) {
            real_call_args.push(vmctx);
            real_call_args.extend_from_slice(call_args);

            // Finally, make the direct call!
            return Ok(builder.ins().call(callee, &real_call_args));
        }

        todo!()
    }

    fn translate_call_indirect(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: TableIndex,
        sig_index: TypeIndex,
        sig_ref: SigRef,
        callee: Value,
        call_args: &[Value],
    ) -> WasmResult<Option<Inst>> {
        todo!()
    }

    fn translate_return_call_indirect(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: TableIndex,
        sig_index: TypeIndex,
        sig_ref: SigRef,
        callee: Value,
        call_args: &[Value],
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_return_call_ref(
        &mut self,
        builder: &mut FunctionBuilder,
        sig_ref: SigRef,
        callee: Value,
        call_args: &[Value],
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_call_ref(
        &mut self,
        builder: &mut FunctionBuilder,
        sig_ref: SigRef,
        callee: Value,
        call_args: &[Value],
    ) -> WasmResult<Inst> {
        todo!()
    }

    fn translate_memory_grow(
        &mut self,
        pos: FuncCursor,
        index: MemoryIndex,
        heap: Heap,
        val: Value,
    ) -> WasmResult<Value> {
        todo!()
    }

    fn translate_memory_size(
        &mut self,
        pos: FuncCursor,
        index: MemoryIndex,
        heap: Heap,
    ) -> WasmResult<Value> {
        todo!()
    }

    fn translate_memory_copy(
        &mut self,
        pos: FuncCursor,
        src_index: MemoryIndex,
        src_heap: Heap,
        dst_index: MemoryIndex,
        dst_heap: Heap,
        dst: Value,
        src: Value,
        len: Value,
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_memory_fill(
        &mut self,
        pos: FuncCursor,
        index: MemoryIndex,
        heap: Heap,
        dst: Value,
        val: Value,
        len: Value,
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_memory_init(
        &mut self,
        pos: FuncCursor,
        index: MemoryIndex,
        heap: Heap,
        seg_index: u32,
        dst: Value,
        src: Value,
        len: Value,
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_data_drop(&mut self, pos: FuncCursor, seg_index: u32) -> WasmResult<()> {
        todo!()
    }

    fn translate_table_size(&mut self, pos: FuncCursor, index: TableIndex) -> WasmResult<Value> {
        todo!()
    }

    fn translate_table_grow(
        &mut self,
        pos: FuncCursor,
        table_index: TableIndex,
        delta: Value,
        init_value: Value,
    ) -> WasmResult<Value> {
        todo!()
    }

    fn translate_table_get(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: TableIndex,
        index: Value,
    ) -> WasmResult<Value> {
        todo!()
    }

    fn translate_table_set(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: TableIndex,
        value: Value,
        index: Value,
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_table_copy(
        &mut self,
        pos: FuncCursor,
        dst_table_index: TableIndex,
        src_table_index: TableIndex,
        dst: Value,
        src: Value,
        len: Value,
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_table_fill(
        &mut self,
        pos: FuncCursor,
        table_index: TableIndex,
        dst: Value,
        val: Value,
        len: Value,
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_table_init(
        &mut self,
        pos: FuncCursor,
        seg_index: u32,
        table_index: TableIndex,
        dst: Value,
        src: Value,
        len: Value,
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_elem_drop(&mut self, pos: FuncCursor, seg_index: u32) -> WasmResult<()> {
        todo!()
    }

    fn translate_ref_null(&mut self, mut pos: FuncCursor, ht: WasmHeapType) -> WasmResult<Value> {
        todo!()
    }

    fn translate_ref_is_null(&mut self, mut pos: FuncCursor, value: Value) -> WasmResult<Value> {
        todo!()
    }

    fn translate_ref_func(&mut self, pos: FuncCursor, func_index: FuncIndex) -> WasmResult<Value> {
        todo!()
    }

    fn translate_custom_global_get(
        &mut self,
        builder: &mut FunctionBuilder,
        global_index: GlobalIndex,
    ) -> WasmResult<Value> {
        todo!()
    }

    fn translate_custom_global_set(
        &mut self,
        builder: &mut FunctionBuilder,
        global_index: GlobalIndex,
        val: Value,
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_atomic_wait(
        &mut self,
        pos: FuncCursor,
        index: MemoryIndex,
        heap: Heap,
        addr: Value,
        expected: Value,
        timeout: Value,
    ) -> WasmResult<Value> {
        todo!()
    }

    fn translate_atomic_notify(
        &mut self,
        pos: FuncCursor,
        index: MemoryIndex,
        heap: Heap,
        addr: Value,
        count: Value,
    ) -> WasmResult<Value> {
        todo!()
    }

    fn translate_ref_i31(&mut self, pos: FuncCursor, val: Value) -> WasmResult<Value> {
        todo!()
    }

    fn translate_i31_get_s(&mut self, pos: FuncCursor, i31ref: Value) -> WasmResult<Value> {
        todo!()
    }

    fn translate_i31_get_u(&mut self, pos: FuncCursor, i31ref: Value) -> WasmResult<Value> {
        todo!()
    }
}
