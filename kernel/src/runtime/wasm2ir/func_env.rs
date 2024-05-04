#![allow(unused)]

use crate::runtime::utils::value_type;
use crate::runtime::vmcontext::VMContextOffsets;
use crate::runtime::wasm2ir::Module;
use crate::runtime::{BuiltinFunctions, WASM_PAGE_SIZE};
use alloc::vec;
use cranelift_codegen::cursor::FuncCursor;
use cranelift_codegen::entity::{EntityRef, PrimaryMap};
use cranelift_codegen::ir::immediates::Offset32;
use cranelift_codegen::ir::types::{I32, I64};
use cranelift_codegen::ir::{
    Fact, FuncRef, Function, GlobalValue, GlobalValueData, Inst, InstBuilder, MemFlags, MemoryType,
    MemoryTypeData, MemoryTypeField, SigRef, Type, Value,
};
use cranelift_codegen::isa::{TargetFrontendConfig, TargetIsa};
use cranelift_frontend::FunctionBuilder;
use cranelift_wasm::wasmparser::UnpackedIndex;
use cranelift_wasm::{
    FuncIndex, GlobalIndex, GlobalVariable, Heap, HeapData, HeapStyle, MemoryIndex, TableIndex,
    TargetEnvironment, TypeConvert, TypeIndex, WasmHeapType, WasmResult,
};

pub struct FuncEnvironment<'module_env, 'wasm> {
    isa: &'module_env dyn TargetIsa,
    module: &'module_env Module<'wasm>,
    pub offsets: VMContextOffsets,
    builtins: BuiltinFunctions,

    heaps: PrimaryMap<Heap, HeapData>,

    vmctx: Option<GlobalValue>,
    pcc_vmctx_memtype: Option<MemoryType>,
}

impl<'module_env, 'wasm> FuncEnvironment<'module_env, 'wasm> {
    pub fn new(isa: &'module_env dyn TargetIsa, module: &'module_env Module<'wasm>) -> Self {
        Self {
            isa,
            module,

            heaps: PrimaryMap::with_capacity(module.memory_plans.len()),
            builtins: BuiltinFunctions::new(isa),
            offsets: VMContextOffsets::for_module(isa, module),

            vmctx: None,
            pcc_vmctx_memtype: None,
        }
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
        let vmctx = self.vmctx(&mut pos.func);
        pos.ins().global_value(pointer_type, vmctx)
    }

    fn get_global_location(
        &mut self,
        func: &mut Function,
        global_index: GlobalIndex,
    ) -> (GlobalValue, i32) {
        // let pointer_type = self.pointer_type();
        let vmctx = self.vmctx(func);
        if let Some(def_index) = self.module.defined_global_index(global_index) {
            let offset = i32::try_from(self.offsets.vmglobal_definition(def_index)).unwrap();
            (vmctx, offset)
        } else {
            todo!("imported memories")
        }
    }

    pub fn cast_memory_index_to_i64(
        &self,
        pos: &mut FuncCursor,
        val: Value,
        memory_index: MemoryIndex,
    ) -> Value {
        if self.memory_index_type(memory_index) == I64 {
            val
        } else {
            pos.ins().uextend(I64, val)
        }
    }

    fn memory_index_type(&self, index: MemoryIndex) -> Type {
        if self.module.memory_plans[index].memory.memory64 {
            I64
        } else {
            I32
        }
    }

    pub fn cast_pointer_to_memory_index(
        &self,
        pos: &mut FuncCursor,
        val: Value,
        memory_index: MemoryIndex,
    ) -> Value {
        let desired_type = self.memory_index_type(memory_index);
        let pointer_type = self.pointer_type();
        assert_eq!(pos.func.dfg.value_type(val), pointer_type);

        // The current length is of type `pointer_type` but we need to fit it
        // into `desired_type`. We are guaranteed that the result will always
        // fit, so we just need to do the right ireduce/sextend here.
        if pointer_type == desired_type {
            val
        } else if pointer_type.bits() > desired_type.bits() {
            pos.ins().ireduce(desired_type, val)
        } else {
            // Note that we `sextend` instead of the probably expected
            // `uextend`. This function is only used within the contexts of
            // `memory.size` and `memory.grow` where we're working with units of
            // pages instead of actual bytes, so we know that the upper bit is
            // always cleared for "valid values". The one case we care about
            // sextend would be when the return value of `memory.grow` is `-1`,
            // in which case we want to copy the sign bit.
            //
            // This should only come up on 32-bit hosts running wasm64 modules,
            // which at some point also makes you question various assumptions
            // made along the way...
            pos.ins().sextend(desired_type, val)
        }
    }
}

impl<'module_env, 'wasm> TargetEnvironment for FuncEnvironment<'module_env, 'wasm> {
    fn target_config(&self) -> TargetFrontendConfig {
        self.isa.frontend_config()
    }

    fn heap_access_spectre_mitigation(&self) -> bool {
        self.isa.flags().enable_heap_access_spectre_mitigation()
    }

    fn proof_carrying_code(&self) -> bool {
        self.isa.flags().enable_pcc()
    }
}

impl<'module_env, 'wasm> TypeConvert for FuncEnvironment<'module_env, 'wasm> {
    fn lookup_heap_type(&self, _index: UnpackedIndex) -> WasmHeapType {
        todo!()
    }
}

impl<'module_env, 'wasm> cranelift_wasm::FuncEnvironment for FuncEnvironment<'module_env, 'wasm> {
    fn make_global(
        &mut self,
        func: &mut Function,
        index: GlobalIndex,
    ) -> WasmResult<GlobalVariable> {
        let ty = self.module.globals[index].wasm_ty;

        if ty.is_vmgcref_type() {
            // Although reference-typed globals live at the same memory location as
            // any other type of global at the same index would, getting or
            // setting them requires ref counting barriers. Therefore, we need
            // to use `GlobalVariable::Custom`, as that is the only kind of
            // `GlobalVariable` for which `cranelift-wasm` supports custom
            // access translation.
            return Ok(GlobalVariable::Custom);
        }

        let (gv, offset) = self.get_global_location(func, index);
        Ok(GlobalVariable::Memory {
            gv,
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

        let bound_bytes = 0x1_0000 * WASM_PAGE_SIZE as u64;

        let def_index = self
            .module
            .defined_memory_index(memory_index)
            .expect("imported memories");
        let owned_index = self.module.owned_memory_index(def_index);
        let base_offset =
            i32::try_from(self.offsets.vmmemory_definition_base(owned_index)).unwrap();

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

        Ok(self.heaps.push(HeapData {
            base,
            min_size,
            max_size,
            offset_guard_size: 0,
            style: HeapStyle::Static { bound: bound_bytes },
            index_type: self.memory_index_type(memory_index),
            memory_type: data_memtype,
        }))
    }

    fn make_indirect_sig(&mut self, _func: &mut Function, _index: TypeIndex) -> WasmResult<SigRef> {
        todo!()
    }

    fn make_direct_func(&mut self, func: &mut Function, index: FuncIndex) -> WasmResult<FuncRef> {
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
        mut pos: FuncCursor,
        index: MemoryIndex,
        _heap: Heap,
        val: Value,
    ) -> WasmResult<Value> {
        let memory_grow = self.builtins.memory32_grow(&mut pos.func);

        let index_arg = index.index();

        let memory_index = pos.ins().iconst(I32, index_arg as i64);
        let vmctx = self.vmctx_val(&mut pos);

        let val = self.cast_memory_index_to_i64(&mut pos, val, index);
        let call_inst = pos.ins().call(memory_grow, &[vmctx, val, memory_index]);
        let result = *pos.func.dfg.inst_results(call_inst).first().unwrap();
        Ok(self.cast_pointer_to_memory_index(&mut pos, result, index))
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

    fn translate_ref_func(&mut self, pos: FuncCursor, func_index: FuncIndex) -> WasmResult<Value> {
        todo!()
    }

    fn translate_custom_global_get(
        &mut self,
        pos: FuncCursor,
        global_index: GlobalIndex,
    ) -> WasmResult<Value> {
        todo!()
    }

    fn translate_custom_global_set(
        &mut self,
        pos: FuncCursor,
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
