use crate::debug::DebugInfo;
use crate::heap::{Heap, HeapData};
use crate::state::GlobalVariable;
use crate::table::{Table, TableData};
use cranelift_codegen::cursor::FuncCursor;
use cranelift_codegen::{ir, isa};
use cranelift_entity::PrimaryMap;
use cranelift_frontend::FunctionBuilder;
use wasmparser::{DataIdx, ElemIdx, FuncIdx, GlobalIdx, MemIdx, TableIdx, TypeIdx};

pub trait TargetEnvironment {
    fn target_config(&self) -> isa::TargetFrontendConfig;

    fn proof_carrying_code(&self) -> bool;

    /// Whether to force relaxed simd instructions to have deterministic
    /// lowerings meaning they will produce the same results across all hosts,
    /// regardless of the cost to performance.
    fn relaxed_simd_deterministic(&self) -> bool {
        true
    }

    /// Whether the target being translated for has a native fma
    /// instruction. If it does not then when relaxed simd isn't deterministic
    /// the translation of the `f32x4.relaxed_fma` instruction, for example,
    /// will do a multiplication and then an add instead of the fused version.
    fn has_native_fma(&self) -> bool {
        false
    }

    /// Returns whether this is an x86 target, which may alter lowerings of
    /// relaxed simd instructions.
    fn is_x86(&self) -> bool {
        false
    }

    /// Returns whether the CLIF `x86_blendv` instruction should be used for the
    /// relaxed simd `*.relaxed_laneselect` instruction for the specified type.
    fn use_x86_blendv_for_relaxed_laneselect(&self, ty: ir::Type) -> bool {
        let _ = ty;
        false
    }

    /// Returns whether the CLIF `x86_pshufb` instruction should be used for the
    /// `i8x16.relaxed_swizzle` instruction.
    fn use_x86_pshufb_for_relaxed_swizzle(&self) -> bool {
        false
    }

    /// Returns whether the CLIF `x86_pmulhrsw` instruction should be used for
    /// the `i8x16.relaxed_q15mulr_s` instruction.
    fn use_x86_pmulhrsw_for_relaxed_q15mul(&self) -> bool {
        false
    }

    /// Returns whether the CLIF `x86_pmaddubsw` instruction should be used for
    /// the relaxed-simd dot-product instructions instruction.
    fn use_x86_pmaddubsw_for_dot(&self) -> bool {
        false
    }
}

/// Environment-specific function translations
pub trait FuncTranslationEnvironment: TargetEnvironment {
    fn lookup_type(&self, type_idx: TypeIdx) -> &wasmparser::FunctionType;

    /// Set up a global variable in `func`.
    ///
    /// `index` is the index of both imported globals and globals defined in the module.
    ///
    /// This method should return the global variable that will be used to access the global and its type.
    fn make_global(&mut self, pos: FuncCursor, index: GlobalIdx) -> crate::Result<GlobalVariable>;

    /// Translate a `global.get` instruction.
    fn translate_global_get(
        &mut self,
        pos: FuncCursor,
        global_index: GlobalIdx,
    ) -> crate::Result<ir::Value>;

    /// Translate a `global.set` instruction.
    fn translate_global_set(
        &mut self,
        pos: FuncCursor,
        global_index: GlobalIdx,
        val: ir::Value,
    ) -> crate::Result<()>;

    fn heaps(&self) -> &PrimaryMap<Heap, HeapData>;

    /// Set up a memory in `func`.
    ///
    /// `index` is the index of both imported memories and memories defined in the module.
    fn make_heap(&mut self, func: &mut ir::Function, index: MemIdx) -> crate::Result<Heap>;

    /// Translate a `memory.init` instruction.
    ///
    /// The `index`/`heap` pair identifies the linear memory to initialize and `data_index` identifies the data segment to copy from.
    fn translate_memory_init(
        &mut self,
        pos: FuncCursor,
        index: MemIdx,
        heap: Heap,
        data_index: DataIdx,
        dst: ir::Value,
        src: ir::Value,
        len: ir::Value,
    ) -> crate::Result<()>;

    /// Translate a `memory.grow` instruction.
    ///
    /// The `index` identifies the linear memory to grow, and `heap` is the heap data for the memory.
    /// The `pages` values is the number of pages to grow by.
    ///
    /// Should return the old size of the memory in pages.
    fn translate_memory_grow(
        &mut self,
        pos: FuncCursor,
        index: MemIdx,
        heap: Heap,
        pages: ir::Value,
    ) -> crate::Result<ir::Value>;

    /// Translate a `memory.size` instruction.
    ///
    /// The `index` identifies the linear memory to query, and `heap` is the heap data for the memory.
    ///
    /// Should return the size of the memory in pages.
    fn translate_memory_size(
        &mut self,
        pos: FuncCursor,
        index: MemIdx,
        heap: Heap,
    ) -> crate::Result<ir::Value>;

    /// Translate a `memory.copy` instruction.
    ///
    /// The `src_index`/`src_heap` and `dst_index`/`dst_heap` identify the linear memories to copy from and to, respectively.
    fn translate_memory_copy(
        &mut self,
        pos: FuncCursor,
        src_index: MemIdx,
        src_heap: Heap,
        dst_index: MemIdx,
        dst_heap: Heap,
        dst: ir::Value,
        src: ir::Value,
        len: ir::Value,
    ) -> crate::Result<()>;

    /// Translate a `memory.fill` instruction.
    ///
    /// The `index`/`heap` pair identifies the linear memory to fill.
    fn translate_memory_fill(
        &mut self,
        pos: FuncCursor,
        index: MemIdx,
        heap: Heap,
        dst: ir::Value,
        val: ir::Value,
        len: ir::Value,
    ) -> crate::Result<()>;

    /// Translate a `data.drop` instruction.
    fn translate_data_drop(&mut self, pos: FuncCursor, data_index: DataIdx) -> crate::Result<()>;

    /// Translate a `memory.discard` instruction from the memory control proposal.
    fn translate_memory_discard(
        &mut self,
        pos: FuncCursor,
        index: MemIdx,
        heap: Heap,
    ) -> crate::Result<()>;

    fn tables(&self) -> &PrimaryMap<Table, TableData>;

    /// Set up a table in `func`.
    fn make_table(&mut self, func: &mut ir::Function, index: TableIdx) -> crate::Result<Table>;

    /// Translate a `table.init` instruction.
    fn translate_table_init(
        &mut self,
        pos: FuncCursor,
        table_index: TableIdx,
        table: Table,
        elem_index: ElemIdx,
        dst: ir::Value,
        src: ir::Value,
        len: ir::Value,
    ) -> crate::Result<()>;

    /// Translate a `table.grow` instruction.
    fn translate_table_grow(
        &mut self,
        pos: FuncCursor,
        table_index: TableIdx,
        table: Table,
        delta: ir::Value,
        init_value: ir::Value,
    ) -> crate::Result<ir::Value>;

    /// Translate a `table.get` instruction.
    fn translate_table_get(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: TableIdx,
        table: Table,
        index: ir::Value,
    ) -> crate::Result<ir::Value>;

    /// Translate a `table.set` instruction.
    fn translate_table_set(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: TableIdx,
        table: Table,
        value: ir::Value,
        index: ir::Value,
    ) -> crate::Result<()>;

    /// Translate a `table.size` instruction.
    fn translate_table_size(
        &mut self,
        pos: FuncCursor,
        index: TableIdx,
        table: Table,
    ) -> crate::Result<ir::Value>;

    /// Translate a `table.copy` instruction.
    fn translate_table_copy(
        &mut self,
        pos: FuncCursor,
        dst_table_index: TableIdx,
        dst_table: Table,
        src_table_index: TableIdx,
        src_table: Table,
        dst: ir::Value,
        src: ir::Value,
        len: ir::Value,
    ) -> crate::Result<()>;

    /// Translate a `table.fill` instruction.
    fn translate_table_fill(
        &mut self,
        pos: FuncCursor,
        table_index: TableIdx,
        table: Table,
        dst: ir::Value,
        val: ir::Value,
        len: ir::Value,
    ) -> crate::Result<()>;

    /// Translate a `elem.drop` instruction.
    fn translate_elem_drop(&mut self, pos: FuncCursor, seg_index: ElemIdx) -> crate::Result<()>;

    // fn translate_atomic_wait(
    //     &mut self,
    //     pos: FuncCursor,
    //     index: MemIdx,
    //     heap: Heap,
    //     addr: ir::Value,
    //     expected: ir::Value,
    //     timeout: ir::Value,
    // ) -> crate::Result<()>;
    // fn translate_atomic_notify(
    //     &mut self,
    //     pos: FuncCursor,
    //     index: MemIdx,
    //     heap: Heap,
    //     addr: ir::Value,
    //     count: ir::Value,
    // ) -> crate::Result<()>;

    fn make_indirect_sig(
        &mut self,
        func: &mut ir::Function,
        index: TypeIdx,
    ) -> crate::Result<ir::SigRef>;

    fn make_direct_func(
        &mut self,
        func: &mut ir::Function,
        index: FuncIdx,
    ) -> crate::Result<ir::FuncRef>;

    fn translate_call(
        &mut self,
        builder: &mut FunctionBuilder,
        callee_index: FuncIdx,
        callee: ir::FuncRef,
        call_args: &[ir::Value],
    ) -> crate::Result<ir::Inst>;

    fn translate_call_indirect(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: TableIdx,
        table: Table,
        sig_index: TypeIdx,
        sig_ref: ir::SigRef,
        callee: ir::Value,
        call_args: &[ir::Value],
    ) -> crate::Result<ir::Inst>;

    fn translate_call_ref(
        &mut self,
        builder: &mut FunctionBuilder,
        sig_ref: ir::SigRef,
        callee: ir::Value,
        call_args: &[ir::Value],
    ) -> crate::Result<ir::Inst>;

    fn translate_return_call(
        &mut self,
        builder: &mut FunctionBuilder,
        _callee_index: FuncIdx,
        callee: ir::FuncRef,
        call_args: &[ir::Value],
    ) -> crate::Result<()>;

    fn translate_return_call_indirect(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: TableIdx,
        table: Table,
        sig_index: TypeIdx,
        sig_ref: ir::SigRef,
        callee: ir::Value,
        call_args: &[ir::Value],
    ) -> crate::Result<()>;

    fn translate_return_call_ref(
        &mut self,
        builder: &mut FunctionBuilder,
        sig_ref: ir::SigRef,
        callee: ir::Value,
        call_args: &[ir::Value],
    ) -> crate::Result<()>;
}

pub trait ModuleTranslationEnvironment<'wasm>: TargetEnvironment {
    fn lookup_type(&self, type_idx: TypeIdx) -> &wasmparser::FunctionType;

    fn reserve_types(&mut self, n: usize) -> crate::Result<()>;
    fn reserve_functions(&mut self, n: usize) -> crate::Result<()>;
    fn reserve_globals(&mut self, n: usize) -> crate::Result<()>;

    fn declare_type(
        &mut self,
        idx: TypeIdx,
        ty: wasmparser::FunctionType<'wasm>,
    ) -> crate::Result<()>;
    fn declare_import(&mut self, import: wasmparser::Import<'wasm>) -> crate::Result<()>;
    fn declare_function(&mut self, idx: FuncIdx, signature: ir::Signature) -> crate::Result<()>;
    fn declare_table(
        &mut self,
        idx: TableIdx,
        table_type: wasmparser::TableType,
    ) -> crate::Result<()>;
    fn declare_memory(
        &mut self,
        idx: MemIdx,
        table_type: wasmparser::MemoryType,
    ) -> crate::Result<()>;
    fn declare_global(
        &mut self,
        idx: GlobalIdx,
        global: wasmparser::Global<'wasm>,
    ) -> crate::Result<()>;
    fn declare_export(&mut self, export: wasmparser::Export<'wasm>) -> crate::Result<()>;
    fn declare_start_function(&mut self, func_idx: FuncIdx) -> crate::Result<()>;
    fn declare_table_element(
        &mut self,
        idx: ElemIdx,
        elem: wasmparser::Element<'wasm>,
    ) -> crate::Result<()>;
    fn declare_function_body(
        &mut self,
        idx: FuncIdx,
        body: wasmparser::FunctionBody<'wasm>,
    ) -> crate::Result<()>;
    fn declare_data_segment(
        &mut self,
        idx: DataIdx,
        data: wasmparser::Data<'wasm>,
    ) -> crate::Result<()>;
    fn declare_debug_info(&mut self, info: DebugInfo<'wasm>) -> crate::Result<()>;
}
