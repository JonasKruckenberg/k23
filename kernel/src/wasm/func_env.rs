use crate::wasm::builtins::BuiltinFunctions;
use crate::wasm::module::{MemoryPlan, MemoryStyle, Module};
use crate::wasm::module_env::ModuleTranslation;
use crate::wasm::utils::value_type;
use crate::wasm::vmcontext::{VMContextOffsets, VMMemoryDefinition};
use crate::wasm::{DEBUG_ASSERT_TRAP_CODE, I31_REF_DISCRIMINANT, WASM_PAGE_SIZE};
use alloc::vec;
use core::mem;
use cranelift_codegen::cursor::FuncCursor;
use cranelift_codegen::entity::PrimaryMap;
use cranelift_codegen::ir;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::immediates::Offset32;
use cranelift_codegen::ir::types::{I32, I64};
use cranelift_codegen::ir::{FuncRef, InstBuilder, SigRef};
use cranelift_codegen::isa::{TargetFrontendConfig, TargetIsa};
use cranelift_frontend::FunctionBuilder;
use cranelift_wasm::wasmparser::UnpackedIndex;
use cranelift_wasm::{
    FuncIndex, FuncTranslationState, GlobalIndex, GlobalVariable, Heap, HeapData, HeapStyle,
    MemoryIndex, TableIndex, TargetEnvironment, TypeConvert, TypeIndex, WasmHeapType, WasmResult,
};

pub struct FuncEnvironment<'module_env, 'wasm> {
    target_isa: &'module_env dyn TargetIsa,
    translation: &'module_env ModuleTranslation<'wasm>,
    offsets: VMContextOffsets,
    builtin_functions: BuiltinFunctions,
    wmemcheck: bool,

    heaps: PrimaryMap<Heap, HeapData>,

    vmctx: Option<ir::GlobalValue>,
    pcc_vmctx_memtype: Option<ir::MemoryType>,
}

impl<'module_env, 'wasm> FuncEnvironment<'module_env, 'wasm> {
    pub fn new(
        target_isa: &'module_env dyn TargetIsa,
        translation: &'module_env ModuleTranslation<'wasm>,
    ) -> Self {
        let offsets = VMContextOffsets::new(&translation.module, target_isa.pointer_bytes() as u32);

        Self {
            target_isa,
            translation,
            offsets,
            builtin_functions: BuiltinFunctions::new(target_isa),
            wmemcheck: true,
            heaps: PrimaryMap::with_capacity(translation.module.memory_plans.len()),
            vmctx: None,
            pcc_vmctx_memtype: None,
        }
    }

    pub fn translate(&mut self, func_index: FuncIndex) {
        todo!()
    }
}

impl<'module_env, 'wasm> FuncEnvironment<'module_env, 'wasm> {
    pub fn vmctx(&mut self, func: &mut ir::Function) -> ir::GlobalValue {
        self.vmctx.unwrap_or_else(|| {
            let vmctx = func.create_global_value(ir::GlobalValueData::VMContext);
            if self.target_isa.flags().enable_pcc() {
                // Create a placeholder memtype for the vmctx; we'll
                // add fields to it as we lazily create HeapData
                // structs and global values.
                let vmctx_memtype = func.create_memory_type(ir::MemoryTypeData::Struct {
                    size: 0,
                    fields: vec![],
                });

                // self.pcc_vmctx_memtype = Some(vmctx_memtype);
                func.global_value_facts[vmctx] = Some(ir::Fact::Mem {
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

    pub fn vmctx_val(&mut self, pos: &mut FuncCursor<'_>) -> ir::Value {
        let pointer_type = self.pointer_type();
        let vmctx = self.vmctx(&mut pos.func);
        pos.ins().global_value(pointer_type, vmctx)
    }

    fn memory_index_type(&self, index: MemoryIndex) -> ir::Type {
        if self.translation.module.memory_plans[index].memory.memory64 {
            I64
        } else {
            I32
        }
    }

    fn cast_pointer_to_memory_index(
        &self,
        mut pos: FuncCursor<'_>,
        val: ir::Value,
        index: MemoryIndex,
    ) -> ir::Value {
        let desired_type = self.memory_index_type(index);
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

    fn cast_memory_index_to_i64(
        &self,
        pos: &mut FuncCursor,
        val: ir::Value,
        memory_index: MemoryIndex,
    ) -> ir::Value {
        if self.memory_index_type(memory_index) == I64 {
            val
        } else {
            pos.ins().uextend(I64, val)
        }
    }

    fn get_memory_atomic_wait(
        &mut self,
        func: &mut ir::Function,
        memory_index: MemoryIndex,
        ty: ir::Type,
    ) -> (ir::FuncRef, u32) {
        match ty {
            I32 => (
                self.builtin_functions.memory_atomic_wait32(func),
                memory_index.as_u32(),
            ),
            I64 => (
                self.builtin_functions.memory_atomic_wait64(func),
                memory_index.as_u32(),
            ),
            x => panic!("get_memory_atomic_wait unsupported type: {:?}", x),
        }
    }

    pub fn get_global_location(
        &mut self,
        func: &mut ir::Function,
        index: GlobalIndex,
    ) -> (ir::GlobalValue, u32) {
        let vmctx = self.vmctx(func);
        if let Some(def_index) = self.translation.module.defined_global_index(index) {
            let offset = self.offsets.vmglobal_definition(def_index);
            (vmctx, offset)
        } else {
            todo!("imported global {index:?}")
        }
    }

    fn check_malloc_start(&mut self, builder: &mut FunctionBuilder) {
        let malloc_start = self.builtin_functions.malloc_start(builder.func);
        let vmctx = self.vmctx_val(&mut builder.cursor());
        builder.ins().call(malloc_start, &[vmctx]);
    }

    fn hook_malloc_exit(&mut self, builder: &mut FunctionBuilder, retvals: &[ir::Value]) {
        let check_malloc = self.builtin_functions.check_malloc(builder.func);
        let vmctx = self.vmctx_val(&mut builder.cursor());
        let func_args = builder
            .func
            .dfg
            .block_params(builder.func.layout.entry_block().unwrap());
        let len = if func_args.len() < 3 {
            return;
        } else {
            // If a function named `malloc` has at least one argument, we assume the
            // first argument is the requested allocation size.
            func_args[2]
        };
        let retval = if retvals.len() < 1 {
            return;
        } else {
            retvals[0]
        };
        builder.ins().call(check_malloc, &[vmctx, retval, len]);
    }

    fn hook_free_exit(&mut self, builder: &mut FunctionBuilder) {
        let check_free = self.builtin_functions.check_free(builder.func);
        let vmctx = self.vmctx_val(&mut builder.cursor());
        let func_args = builder
            .func
            .dfg
            .block_params(builder.func.layout.entry_block().unwrap());
        let ptr = if func_args.len() < 3 {
            return;
        } else {
            // If a function named `free` has at least one argument, we assume the
            // first argument is a pointer to memory.
            func_args[2]
        };
        builder.ins().call(check_free, &[vmctx, ptr]);
    }

    fn check_free_start(&mut self, builder: &mut FunctionBuilder) {
        let free_start = self.builtin_functions.free_start(builder.func);
        let vmctx = self.vmctx_val(&mut builder.cursor());
        builder.ins().call(free_start, &[vmctx]);
    }

    fn current_func_name(&self, builder: &mut FunctionBuilder) -> Option<&str> {
        let func_index = match &builder.func.name {
            ir::UserFuncName::User(user) => FuncIndex::from_u32(user.index),
            _ => {
                panic!("function name not a UserFuncName::User as expected")
            }
        };
        self.translation
            .debuginfo
            .name_section
            .func_names
            .get(&func_index)
            .map(|s| *s)
    }

    fn i31_ref_to_unshifted_value(&self, pos: &mut FuncCursor, i31ref: ir::Value) -> ir::Value {
        let ref_ty = self.reference_type(WasmHeapType::I31);
        debug_assert_eq!(pos.func.dfg.value_type(i31ref), ref_ty);

        let is_null = pos.ins().is_null(i31ref);
        pos.ins().trapnz(is_null, ir::TrapCode::NullI31Ref);

        let val = pos
            .ins()
            .bitcast(ref_ty.as_int(), ir::MemFlags::new(), i31ref);

        if cfg!(debug_assertions) {
            let is_i31_ref = pos.ins().band_imm(val, i64::from(I31_REF_DISCRIMINANT));
            pos.ins()
                .trapz(is_i31_ref, ir::TrapCode::User(DEBUG_ASSERT_TRAP_CODE));
        }

        match ref_ty.bytes() {
            8 => pos.ins().ireduce(ir::types::I32, val),
            4 => val,
            _ => unreachable!(),
        }
    }
}

impl<'module_env, 'wasm> TypeConvert for FuncEnvironment<'module_env, 'wasm> {
    fn lookup_heap_type(&self, ty: UnpackedIndex) -> WasmHeapType {
        todo!()
    }
}

impl<'module_env, 'wasm> TargetEnvironment for FuncEnvironment<'module_env, 'wasm> {
    fn target_config(&self) -> TargetFrontendConfig {
        self.target_isa.frontend_config()
    }

    fn heap_access_spectre_mitigation(&self) -> bool {
        self.target_isa
            .flags()
            .enable_heap_access_spectre_mitigation()
    }

    fn proof_carrying_code(&self) -> bool {
        self.target_isa.flags().enable_pcc()
    }
}

impl<'module_env, 'wasm> cranelift_wasm::FuncEnvironment for FuncEnvironment<'module_env, 'wasm> {
    fn is_x86(&self) -> bool {
        self.target_isa.triple().architecture == target_lexicon::Architecture::X86_64
    }

    fn has_native_fma(&self) -> bool {
        self.target_isa.has_native_fma()
    }

    fn use_x86_blendv_for_relaxed_laneselect(&self, ty: ir::Type) -> bool {
        self.target_isa.has_x86_blendv_lowering(ty)
    }

    fn use_x86_pshufb_for_relaxed_swizzle(&self) -> bool {
        self.target_isa.has_x86_pshufb_lowering()
    }

    fn use_x86_pmulhrsw_for_relaxed_q15mul(&self) -> bool {
        self.target_isa.has_x86_pmulhrsw_lowering()
    }

    fn use_x86_pmaddubsw_for_dot(&self) -> bool {
        self.target_isa.has_x86_pmaddubsw_lowering()
    }

    fn make_global(
        &mut self,
        func: &mut ir::Function,
        index: GlobalIndex,
    ) -> WasmResult<GlobalVariable> {
        let ty = self.translation.module.globals[index].wasm_ty;

        if ty.is_vmgcref_type() {
            todo!("gc types")
        }

        let (gv, offset) = self.get_global_location(func, index);

        Ok(GlobalVariable::Memory {
            gv,
            offset: Offset32::new(offset as i32),
            ty: value_type(self.target_isa, ty),
        })
    }

    fn heaps(&self) -> &PrimaryMap<Heap, HeapData> {
        &self.heaps
    }

    fn make_heap(&mut self, func: &mut ir::Function, index: MemoryIndex) -> WasmResult<Heap> {
        let pointer_type = self.pointer_type();

        let min_size = self.translation.module.memory_plans[index]
            .memory
            .minimum
            .checked_mul(u64::from(WASM_PAGE_SIZE))
            .unwrap_or_else(|| {
                // The only valid Wasm memory size that won't fit in a 64-bit
                // integer is the maximum memory64 size (2^64) which is one
                // larger than `u64::MAX` (2^64 - 1). In this case, just say the
                // minimum heap size is `u64::MAX`.
                debug_assert_eq!(
                    self.translation.module.memory_plans[index].memory.minimum,
                    1 << 48
                );
                u64::MAX
            });

        let max_size = self.translation.module.memory_plans[index]
            .memory
            .maximum
            .and_then(|max| max.checked_mul(u64::from(WASM_PAGE_SIZE)));

        let def_index = self
            .translation
            .module
            .defined_memory_index(index)
            .expect("TODO: imported memories");
        assert!(
            !self.translation.module.memory_plans[index].memory.shared,
            "TODO: shared memories"
        );

        let ptr = self.vmctx(func);

        let owned_index = self.translation.module.owned_memory_index(def_index);
        let owned_base_offset = self.offsets.vmmemory_definition(owned_index)
            + mem::offset_of!(VMMemoryDefinition, base) as u32;
        let base_offset = i32::try_from(owned_base_offset).unwrap();

        let (heap_style, base_fact, memory_type) = match self.translation.module.memory_plans[index]
        {
            MemoryPlan {
                style: MemoryStyle::Static { max_pages },
                ..
            } => {
                let bound_bytes = u64::from(max_pages) * u64::from(WASM_PAGE_SIZE);

                let (base_fact, data_memtype) = if let Some(ptr_memtype) = self.pcc_vmctx_memtype {
                    // Create a memtype representing the untyped memory region.
                    let data_memtype =
                        func.create_memory_type(ir::MemoryTypeData::Memory { size: bound_bytes });
                    // This fact applies to any pointer to the start of the memory.
                    let base_fact = ir::Fact::Mem {
                        ty: data_memtype,
                        min_offset: 0,
                        max_offset: 0,
                        nullable: false,
                    };
                    // Create a field in the vmctx for the base pointer.
                    match &mut func.memory_types[ptr_memtype] {
                        ir::MemoryTypeData::Struct { size, fields } => {
                            let offset = u64::try_from(base_offset).unwrap();
                            fields.push(ir::MemoryTypeField {
                                offset,
                                ty: self.target_isa.pointer_type(),
                                // Read-only field from the PoV of PCC checks:
                                // don't allow stores to this field. (Even if
                                // it is a dynamic memory whose base can
                                // change, that update happens inside the
                                // runtime, not in generated code.)
                                readonly: true,
                                fact: Some(base_fact.clone()),
                            });
                            *size = core::cmp::max(
                                *size,
                                offset + u64::from(self.target_isa.pointer_type().bytes()),
                            );
                        }
                        _ => {
                            panic!("Bad memtype");
                        }
                    }

                    (Some(base_fact), Some(data_memtype))
                } else {
                    (None, None)
                };

                (
                    HeapStyle::Static { bound: bound_bytes },
                    base_fact,
                    data_memtype,
                )
            }
            MemoryPlan {
                style: MemoryStyle::Dynamic,
                ..
            } => todo!("dynamic memory"),
        };

        let mut base_flags = ir::MemFlags::trusted().with_checked();
        base_flags.set_readonly();

        let heap_base = func.create_global_value(ir::GlobalValueData::Load {
            base: ptr,
            offset: Offset32::new(base_offset),
            global_type: pointer_type,
            flags: base_flags,
        });
        func.global_value_facts[heap_base] = base_fact;

        Ok(self.heaps.push(HeapData {
            base: heap_base,
            min_size,
            max_size,
            offset_guard_size: 0,
            style: heap_style,
            index_type: self.memory_index_type(index),
            memory_type,
        }))
    }

    fn make_indirect_sig(
        &mut self,
        _func: &mut ir::Function,
        _index: TypeIndex,
    ) -> WasmResult<SigRef> {
        todo!()
    }

    fn make_direct_func(
        &mut self,
        _func: &mut ir::Function,
        _index: FuncIndex,
    ) -> WasmResult<FuncRef> {
        todo!()
    }

    fn translate_call_indirect(
        &mut self,
        _builder: &mut FunctionBuilder,
        _table_index: TableIndex,
        _sig_index: TypeIndex,
        _sig_ref: SigRef,
        _callee: ir::Value,
        _call_args: &[ir::Value],
    ) -> WasmResult<Option<ir::Inst>> {
        todo!()
    }

    fn translate_return_call_indirect(
        &mut self,
        _builder: &mut FunctionBuilder,
        _table_index: TableIndex,
        _sig_index: TypeIndex,
        _sig_ref: SigRef,
        _callee: ir::Value,
        _call_args: &[ir::Value],
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_return_call_ref(
        &mut self,
        _builder: &mut FunctionBuilder,
        _sig_ref: SigRef,
        _callee: ir::Value,
        _call_args: &[ir::Value],
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_call_ref(
        &mut self,
        _builder: &mut FunctionBuilder,
        _sig_ref: SigRef,
        _callee: ir::Value,
        _call_args: &[ir::Value],
    ) -> WasmResult<ir::Inst> {
        todo!()
    }

    fn translate_memory_grow(
        &mut self,
        mut pos: FuncCursor,
        memory_index: MemoryIndex,
        _heap: Heap,
        val: ir::Value,
    ) -> WasmResult<ir::Value> {
        // memory32_grow(vmctx: vmctx, delta: i64, index: i32) -> pointer;
        let memory_grow = self.builtin_functions.memory32_grow(&mut pos.func);

        let vmctx = self.vmctx_val(&mut pos);
        let index = pos.ins().iconst(I32, i64::from(memory_index.as_u32()));
        let val = self.cast_memory_index_to_i64(&mut pos, val, memory_index);
        let call_inst = pos.ins().call(memory_grow, &[vmctx, val, index]);

        let result = pos.func.dfg.first_result(call_inst);
        Ok(self.cast_pointer_to_memory_index(pos, result, memory_index))
    }

    fn translate_memory_size(
        &mut self,
        _pos: FuncCursor,
        _index: MemoryIndex,
        _heap: Heap,
    ) -> WasmResult<ir::Value> {
        todo!()
    }

    fn translate_memory_copy(
        &mut self,
        mut pos: FuncCursor,
        src_index: MemoryIndex,
        _src_heap: Heap,
        dst_index: MemoryIndex,
        _dst_heap: Heap,
        dst: ir::Value,
        src: ir::Value,
        len: ir::Value,
    ) -> WasmResult<()> {
        // memory_copy(vmctx: vmctx, dst_index: i32, dst: i64, src_index: i32, src: i64, len: i64);
        let memory_copy = self.builtin_functions.memory_copy(&mut pos.func);

        // The length is 32-bit if either memory is 32-bit, but if they're both
        // 64-bit then it's 64-bit. Our intrinsic takes a 64-bit length for
        // compatibility across all memories, so make sure that it's cast
        // correctly here (this is a bit special so no generic helper unlike for
        // `dst`/`src` above)
        let len = if self.memory_index_type(dst_index) == I64
            && self.memory_index_type(src_index) == I64
        {
            len
        } else {
            pos.ins().uextend(I64, len)
        };

        let vmctx = self.vmctx_val(&mut pos);
        let dst = self.cast_memory_index_to_i64(&mut pos, dst, dst_index);
        let dst_index = pos.ins().iconst(I32, i64::from(dst_index.as_u32()));
        let src = self.cast_memory_index_to_i64(&mut pos, src, src_index);
        let src_index = pos.ins().iconst(I32, i64::from(src_index.as_u32()));

        pos.ins()
            .call(memory_copy, &[vmctx, dst_index, dst, src_index, src, len]);

        Ok(())
    }

    fn translate_memory_fill(
        &mut self,
        mut pos: FuncCursor,
        memory_index: MemoryIndex,
        _heap: Heap,
        dst: ir::Value,
        val: ir::Value,
        len: ir::Value,
    ) -> WasmResult<()> {
        // memory_fill(vmctx: vmctx, memory: i32, dst: i64, val: i32, len: i64);
        let memory_fill = self.builtin_functions.memory_fill(&mut pos.func);

        let vmctx = self.vmctx_val(&mut pos);
        let memory = pos.ins().iconst(I32, i64::from(memory_index.as_u32()));
        let dst = self.cast_memory_index_to_i64(&mut pos, dst, memory_index);
        let len = self.cast_memory_index_to_i64(&mut pos, len, memory_index);

        pos.ins().call(memory_fill, &[vmctx, memory, dst, val, len]);

        Ok(())
    }

    fn translate_memory_init(
        &mut self,
        mut pos: FuncCursor,
        memory_index: MemoryIndex,
        _heap: Heap,
        seg_index: u32,
        dst: ir::Value,
        src: ir::Value,
        len: ir::Value,
    ) -> WasmResult<()> {
        // memory_init(vmctx: vmctx, memory: i32, data: i32, dst: i64, src: i32, len: i32);
        let memory_init = self.builtin_functions.memory_init(&mut pos.func);

        let vmctx = self.vmctx_val(&mut pos);
        let memory = pos.ins().iconst(I32, i64::from(memory_index.as_u32()));
        let data = pos.ins().iconst(I32, i64::from(seg_index));
        let dst = self.cast_memory_index_to_i64(&mut pos, dst, memory_index);

        pos.ins()
            .call(memory_init, &[vmctx, memory, data, dst, src, len]);

        Ok(())
    }

    fn translate_data_drop(&mut self, mut pos: FuncCursor, seg_index: u32) -> WasmResult<()> {
        // data_drop(vmctx: vmctx, data: i32);
        let data_drop = self.builtin_functions.data_drop(&mut pos.func);

        let vmctx = self.vmctx_val(&mut pos);
        let seg_index = pos.ins().iconst(I32, i64::from(seg_index));

        pos.ins().call(data_drop, &[vmctx, seg_index]);

        Ok(())
    }

    fn translate_table_size(
        &mut self,
        _pos: FuncCursor,
        _index: TableIndex,
    ) -> WasmResult<ir::Value> {
        todo!()
    }

    fn translate_table_grow(
        &mut self,
        _pos: FuncCursor,
        _table_index: TableIndex,
        _delta: ir::Value,
        _init_value: ir::Value,
    ) -> WasmResult<ir::Value> {
        todo!()
    }

    fn translate_table_get(
        &mut self,
        _builder: &mut FunctionBuilder,
        _table_index: TableIndex,
        _index: ir::Value,
    ) -> WasmResult<ir::Value> {
        todo!()
    }

    fn translate_table_set(
        &mut self,
        _builder: &mut FunctionBuilder,
        _table_index: TableIndex,
        _value: ir::Value,
        _index: ir::Value,
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_table_copy(
        &mut self,
        mut pos: FuncCursor,
        dst_table_index: TableIndex,
        src_table_index: TableIndex,
        dst: ir::Value,
        src: ir::Value,
        len: ir::Value,
    ) -> WasmResult<()> {
        // table_copy(vmctx: vmctx, dst_index: i32, src_index: i32, dst: i32, src: i32, len: i32);
        let table_copy = self.builtin_functions.table_copy(&mut pos.func);

        let vmctx = self.vmctx_val(&mut pos);
        let dst_index = pos.ins().iconst(I32, i64::from(dst_table_index.as_u32()));
        let src_index = pos.ins().iconst(I32, i64::from(src_table_index.as_u32()));

        pos.ins()
            .call(table_copy, &[vmctx, dst_index, src_index, dst, src, len]);

        Ok(())
    }

    fn translate_table_fill(
        &mut self,
        _pos: FuncCursor,
        _table_index: TableIndex,
        _dst: ir::Value,
        _val: ir::Value,
        _len: ir::Value,
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_table_init(
        &mut self,
        mut pos: FuncCursor,
        seg_index: u32,
        table_index: TableIndex,
        dst: ir::Value,
        src: ir::Value,
        len: ir::Value,
    ) -> WasmResult<()> {
        // table_init(vmctx: vmctx, table: i32, elem: i32, dst: i32, src: i32, len: i32);
        let table_init = self.builtin_functions.table_init(&mut pos.func);

        let vmctx = self.vmctx_val(&mut pos);
        let table = pos.ins().iconst(I32, i64::from(table_index.as_u32()));
        let elem = pos.ins().iconst(I32, i64::from(seg_index));

        pos.ins()
            .call(table_init, &[vmctx, table, elem, dst, src, len]);

        Ok(())
    }

    fn translate_elem_drop(&mut self, mut pos: FuncCursor, seg_index: u32) -> WasmResult<()> {
        // elem_drop(vmctx: vmctx, elem: i32);
        let elem_drop = self.builtin_functions.elem_drop(&mut pos.func);

        let vmctx = self.vmctx_val(&mut pos);
        let elem = pos.ins().iconst(I32, i64::from(seg_index));

        pos.ins().call(elem_drop, &[vmctx, elem]);

        Ok(())
    }

    fn translate_custom_global_get(
        &mut self,
        _pos: FuncCursor,
        _global_index: GlobalIndex,
    ) -> WasmResult<ir::Value> {
        todo!()
    }

    fn translate_custom_global_set(
        &mut self,
        _pos: FuncCursor,
        _global_index: GlobalIndex,
        _val: ir::Value,
    ) -> WasmResult<()> {
        todo!()
    }

    fn translate_atomic_wait(
        &mut self,
        mut pos: FuncCursor,
        memory_index: MemoryIndex,
        _heap: Heap,
        addr: ir::Value,
        expected: ir::Value,
        timeout: ir::Value,
    ) -> WasmResult<ir::Value> {
        let implied_ty = pos.func.dfg.value_type(expected);

        // memory_atomic_wait32(vmctx: vmctx, memory: i32, addr: i64, expected: i32, timeout: i64) -> i32;
        // memory_atomic_wait64(vmctx: vmctx, memory: i32, addr: i64, expected: i64, timeout: i64) -> i32;
        let (memory_atomic_wait, memory) =
            self.get_memory_atomic_wait(&mut pos.func, memory_index, implied_ty);

        let vmctx = self.vmctx_val(&mut pos);
        let memory = pos.ins().iconst(I32, i64::from(memory));
        let addr = self.cast_memory_index_to_i64(&mut pos, addr, memory_index);

        let call_inst = pos.ins().call(
            memory_atomic_wait,
            &[vmctx, memory, addr, expected, timeout],
        );
        Ok(pos.func.dfg.first_result(call_inst))
    }

    fn translate_atomic_notify(
        &mut self,
        mut pos: FuncCursor,
        index: MemoryIndex,
        _heap: Heap,
        addr: ir::Value,
        count: ir::Value,
    ) -> WasmResult<ir::Value> {
        // memory_atomic_notify(vmctx: vmctx, memory: i32, addr: i64, count: i32) -> i32;
        let memory_atomic_notify = self.builtin_functions.memory_atomic_notify(&mut pos.func);

        let vmctx = self.vmctx_val(&mut pos);
        let memory = pos.ins().iconst(I32, i64::from(index.as_u32()));

        let call_inst = pos
            .ins()
            .call(memory_atomic_notify, &[vmctx, memory, addr, count]);
        Ok(pos.func.dfg.first_result(call_inst))
    }

    fn translate_ref_i31(&mut self, mut pos: FuncCursor, val: ir::Value) -> WasmResult<ir::Value> {
        let shifted = pos.ins().ishl_imm(val, 1);
        let tagged = pos.ins().bor_imm(shifted, i64::from(I31_REF_DISCRIMINANT));
        let ref_ty = self.reference_type(WasmHeapType::I31);
        let extended = if ref_ty.bytes() > 4 {
            pos.ins().uextend(ref_ty.as_int(), tagged)
        } else {
            tagged
        };
        let i31ref = pos.ins().bitcast(ref_ty, ir::MemFlags::new(), extended);
        Ok(i31ref)
    }

    fn translate_i31_get_s(
        &mut self,
        mut pos: FuncCursor,
        i31ref: ir::Value,
    ) -> WasmResult<ir::Value> {
        let val = self.i31_ref_to_unshifted_value(&mut pos, i31ref);
        let shifted = pos.ins().sshr_imm(val, 1);
        Ok(shifted)
    }

    fn translate_i31_get_u(
        &mut self,
        mut pos: FuncCursor,
        i31ref: ir::Value,
    ) -> WasmResult<ir::Value> {
        let val = self.i31_ref_to_unshifted_value(&mut pos, i31ref);
        let shifted = pos.ins().ushr_imm(val, 1);
        Ok(shifted)
    }

    fn translate_ref_null(
        &mut self,
        mut pos: FuncCursor,
        ht: WasmHeapType,
    ) -> WasmResult<ir::Value> {
        Ok(match ht {
            WasmHeapType::Func | WasmHeapType::ConcreteFunc(_) | WasmHeapType::NoFunc => {
                pos.ins().iconst(self.pointer_type(), 0)
            }
            WasmHeapType::Extern | WasmHeapType::Any | WasmHeapType::I31 | WasmHeapType::None => {
                pos.ins().null(self.reference_type(ht))
            }
        })
    }

    fn translate_ref_is_null(
        &mut self,
        mut pos: FuncCursor,
        value: ir::Value,
    ) -> WasmResult<ir::Value> {
        let bool_is_null = match pos.func.dfg.value_type(value) {
            // `externref`
            ty if ty.is_ref() => pos.ins().is_null(value),
            // `funcref`
            ty if ty == self.pointer_type() => pos.ins().icmp_imm(IntCC::Equal, value, 0),
            _ => unreachable!(),
        };

        Ok(pos.ins().uextend(I32, bool_is_null))
    }

    fn translate_ref_func(
        &mut self,
        mut pos: FuncCursor,
        func_index: FuncIndex,
    ) -> WasmResult<ir::Value> {
        // ref_func(vmctx: vmctx, func: i32) -> pointer;
        let ref_func = self.builtin_functions.ref_func(&mut pos.func);

        let vmctx = self.vmctx_val(&mut pos);
        let func = pos.ins().iconst(I32, i64::from(func_index.as_u32()));

        let call_inst = pos.ins().call(ref_func, &[vmctx, func]);
        Ok(pos.func.dfg.first_result(call_inst))
    }

    fn before_translate_function(
        &mut self,
        builder: &mut FunctionBuilder,
        _state: &FuncTranslationState,
    ) -> WasmResult<()> {
        if self.wmemcheck {
            let func_name = self.current_func_name(builder);
            if func_name == Some("malloc") {
                self.check_malloc_start(builder);
            } else if func_name == Some("free") {
                self.check_free_start(builder);
            }
        }
        Ok(())
    }

    fn handle_before_return(&mut self, retvals: &[ir::Value], builder: &mut FunctionBuilder) {
        if self.wmemcheck {
            let func_name = self.current_func_name(builder);
            if func_name == Some("malloc") {
                self.hook_malloc_exit(builder, retvals);
            } else if func_name == Some("free") {
                self.hook_free_exit(builder);
            }
        }
    }

    fn before_load(
        &mut self,
        builder: &mut FunctionBuilder,
        val_size: u8,
        addr: ir::Value,
        offset: u64,
    ) {
        if self.wmemcheck {
            let check_load = self.builtin_functions.check_load(builder.func);
            let vmctx = self.vmctx_val(&mut builder.cursor());
            let num_bytes = builder.ins().iconst(I32, val_size as i64);
            let offset_val = builder.ins().iconst(I64, offset as i64);
            builder
                .ins()
                .call(check_load, &[vmctx, num_bytes, addr, offset_val]);
        }
    }

    fn before_store(
        &mut self,
        builder: &mut FunctionBuilder,
        val_size: u8,
        addr: ir::Value,
        offset: u64,
    ) {
        if self.wmemcheck {
            let check_store = self.builtin_functions.check_store(builder.func);
            let vmctx = self.vmctx_val(&mut builder.cursor());
            let num_bytes = builder.ins().iconst(I32, val_size as i64);
            let offset_val = builder.ins().iconst(I64, offset as i64);
            builder
                .ins()
                .call(check_store, &[vmctx, num_bytes, addr, offset_val]);
        }
    }

    fn update_global(
        &mut self,
        builder: &mut FunctionBuilder,
        global_index: u32,
        value: ir::Value,
    ) {
        if self.wmemcheck {
            if global_index == 0 {
                // We are making the assumption that global 0 is the auxiliary stack pointer.
                // TODO use debug info to figure out correct stack ptr
                let update_stack_pointer =
                    self.builtin_functions.update_stack_pointer(builder.func);
                let vmctx = self.vmctx_val(&mut builder.cursor());
                builder.ins().call(update_stack_pointer, &[vmctx, value]);
            }
        }
    }

    fn before_memory_grow(
        &mut self,
        builder: &mut FunctionBuilder,
        num_bytes: ir::Value,
        mem_index: MemoryIndex,
    ) {
        if self.wmemcheck && mem_index.as_u32() == 0 {
            let update_mem_size = self.builtin_functions.update_mem_size(builder.func);
            let vmctx = self.vmctx_val(&mut builder.cursor());
            builder.ins().call(update_mem_size, &[vmctx, num_bytes]);
        }
    }
}
