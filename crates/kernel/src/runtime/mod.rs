mod host_calls;
mod vmctx;

use alloc::vec;
use core::mem;
use core::ops::Range;
use cranelift_codegen::cursor::FuncCursor;
use cranelift_codegen::entity::{PrimaryMap, SecondaryMap};
use cranelift_codegen::ir;
use cranelift_codegen::ir::immediates::{Imm64, Offset32, Uimm64};
use cranelift_codegen::ir::{
    types, GlobalValue, GlobalValueData, InstBuilder, MemoryTypeData, TableData, Type,
};
use cranelift_codegen::isa::{TargetFrontendConfig, TargetIsa};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_wasm::{GlobalVariable, HeapData, TargetEnvironment};
use wasmparser::{Limits, Mutability, ReferenceType, ValueType};

const MAX_MEMORY_SIZE: u64 = 0x0000003fffffffff;

pub fn compile_wasm(bytes: &[u8]) {
    let compiler = cranelift_wasm::Compiler::new_for_host().unwrap();

    let mut env = Environment::new(compiler.target_isa(), MAX_MEMORY_SIZE);

    let module = wasmparser::parse_module(bytes).unwrap();

    cranelift_wasm::translate_module(module, &mut env).unwrap();

    let translation = env.finish();

    let mut iter = translation.functions.iter();
    while let Some((_idx, Some(func))) = iter.next() {
        log::debug!("translated func {}", func.display());

        let compiled_func = compiler.compile_function(func).unwrap();
    }
}

#[derive(Default)]
struct ModuleTranslation<'wasm> {
    functions: SecondaryMap<wasmparser::FuncIdx, Option<ir::Function>>,
    start_func: Option<wasmparser::FuncIdx>,
    debug_info: Option<cranelift_wasm::DebugInfo<'wasm>>,
}

struct Environment<'a, 'wasm> {
    func_builder_ctx: FunctionBuilderContext,
    func_translation_state: cranelift_wasm::State,
    func_translation_env: FunctionEnvironment<'a, 'wasm>,
    signatures: SecondaryMap<wasmparser::FuncIdx, Option<ir::Signature>>,

    result: ModuleTranslation<'wasm>,
}

impl<'a, 'wasm> Environment<'a, 'wasm> {
    pub fn new(target_isa: &'a dyn TargetIsa, memory_max_size: u64) -> Self {
        Self {
            func_builder_ctx: FunctionBuilderContext::new(),
            func_translation_state: cranelift_wasm::State::new(),
            func_translation_env: FunctionEnvironment::new(target_isa, memory_max_size),
            signatures: SecondaryMap::new(),
            result: ModuleTranslation::default(),
        }
    }

    pub fn finish(&mut self) -> ModuleTranslation {
        self.signatures.clear();

        mem::take(&mut self.result)
    }
}

impl<'a, 'wasm> TargetEnvironment for Environment<'a, 'wasm> {
    fn target_config(&self) -> TargetFrontendConfig {
        self.func_translation_env.target_isa.frontend_config()
    }

    fn proof_carrying_code(&self) -> bool {
        true
    }
}

impl<'a, 'wasm> cranelift_wasm::ModuleTranslationEnvironment<'wasm> for Environment<'a, 'wasm> {
    fn lookup_type(&self, type_idx: wasmparser::TypeIdx) -> &wasmparser::FunctionType {
        &self.func_translation_env.types[type_idx].as_ref().unwrap()
    }

    fn reserve_types(&mut self, n: usize) -> Result<(), cranelift_wasm::Error> {
        // self.types.reserve(n);
        Ok(())
    }

    fn reserve_functions(&mut self, n: usize) -> Result<(), cranelift_wasm::Error> {
        self.signatures.resize(n);
        self.result.functions.resize(n);
        Ok(())
    }

    fn reserve_globals(&mut self, n: usize) -> Result<(), cranelift_wasm::Error> {
        self.func_translation_env.globals.resize(n);
        Ok(())
    }

    fn declare_type(
        &mut self,
        idx: wasmparser::TypeIdx,
        ty: wasmparser::FunctionType<'wasm>,
    ) -> Result<(), cranelift_wasm::Error> {
        self.func_translation_env.types[idx] = Some(ty);
        Ok(())
    }

    fn declare_import(&mut self, import: wasmparser::Import) -> Result<(), cranelift_wasm::Error> {
        log::debug!("TODO: implement import");
        Ok(())
    }

    fn declare_function(
        &mut self,
        idx: wasmparser::FuncIdx,
        signature: ir::Signature,
    ) -> Result<(), cranelift_wasm::Error> {
        self.signatures[idx] = Some(signature);
        Ok(())
    }

    fn declare_table(
        &mut self,
        idx: wasmparser::TableIdx,
        table_type: wasmparser::TableType,
    ) -> Result<(), cranelift_wasm::Error> {
        log::debug!("TODO: implement table");
        Ok(())
    }

    fn declare_memory(
        &mut self,
        idx: wasmparser::MemIdx,
        memory_type: wasmparser::MemoryType,
    ) -> Result<(), cranelift_wasm::Error> {
        self.func_translation_env.memories[idx] = Some(memory_type);
        Ok(())
    }

    fn declare_global(
        &mut self,
        idx: wasmparser::GlobalIdx,
        global: wasmparser::Global<'wasm>,
    ) -> Result<(), cranelift_wasm::Error> {
        self.func_translation_env.globals[idx] = Some(global);
        Ok(())
    }

    fn declare_export(&mut self, export: wasmparser::Export) -> Result<(), cranelift_wasm::Error> {
        log::debug!("TODO: implement import");
        Ok(())
    }

    fn declare_start_function(
        &mut self,
        func_idx: wasmparser::FuncIdx,
    ) -> Result<(), cranelift_wasm::Error> {
        self.result.start_func = Some(func_idx);
        Ok(())
    }

    fn declare_table_element(
        &mut self,
        idx: wasmparser::ElemIdx,
        elem: wasmparser::Element,
    ) -> Result<(), cranelift_wasm::Error> {
        log::debug!("TODO: implement table element");
        Ok(())
    }

    fn declare_function_body(
        &mut self,
        idx: wasmparser::FuncIdx,
        body: wasmparser::FunctionBody,
    ) -> Result<(), cranelift_wasm::Error> {
        let sig = self.signatures[idx].clone().unwrap();

        let func = cranelift_wasm::translate_function(
            &mut self.func_translation_state,
            &mut self.func_builder_ctx,
            idx,
            sig,
            body,
            &mut self.func_translation_env,
        )?;

        self.result.functions[idx] = Some(func);

        Ok(())
    }

    fn declare_data_segment(
        &mut self,
        idx: wasmparser::DataIdx,
        data: wasmparser::Data,
    ) -> Result<(), cranelift_wasm::Error> {
        log::debug!("TODO: implement data segment");
        Ok(())
    }

    fn declare_debug_info(
        &mut self,
        info: cranelift_wasm::DebugInfo<'wasm>,
    ) -> Result<(), cranelift_wasm::Error> {
        self.result.debug_info = Some(info);
        Ok(())
    }
}

struct FunctionEnvironment<'a, 'wasm> {
    target_isa: &'a dyn TargetIsa,
    memory_max_size: u64,

    types: SecondaryMap<wasmparser::TypeIdx, Option<wasmparser::FunctionType<'wasm>>>,
    globals: SecondaryMap<wasmparser::GlobalIdx, Option<wasmparser::Global<'wasm>>>,
    memories: SecondaryMap<wasmparser::MemIdx, Option<wasmparser::MemoryType>>,
    tables: SecondaryMap<wasmparser::TableIdx, Option<wasmparser::TableType>>,

    heaps: PrimaryMap<cranelift_wasm::Heap, HeapData>,
}

enum ConstExprResult {
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
}

impl<'a, 'wasm> FunctionEnvironment<'a, 'wasm> {
    pub fn new(target_isa: &'a dyn TargetIsa, memory_max_size: u64) -> Self {
        Self {
            target_isa,
            memory_max_size,
            types: SecondaryMap::new(),
            globals: SecondaryMap::new(),
            memories: SecondaryMap::new(),
            tables: SecondaryMap::new(),
            heaps: PrimaryMap::new(),
        }
    }

    pub fn get_heap_base(&self) -> GlobalValue {
        todo!()
    }

    pub fn get_tables_base(&self) -> GlobalValue {
        todo!()
    }

    pub fn get_globals_base(&self) -> GlobalValue {
        todo!()
    }

    fn eval_const_expr(
        &self,
        expr: &wasmparser::ConstExpr,
    ) -> Result<ConstExprResult, wasmparser::Error> {
        use wasmparser::Instruction;

        let mut stack = vec![];

        for inst in expr.instructions() {
            match inst? {
                Instruction::End => break,
                Instruction::I32Const { value } => stack.push(ConstExprResult::I32(value)),
                Instruction::I64Const { value } => stack.push(ConstExprResult::I64(value)),
                Instruction::F32Const { value } => stack.push(ConstExprResult::F32(value.as_f32())),
                Instruction::F64Const { value } => stack.push(ConstExprResult::F64(value.as_f64())),
                Instruction::RefNull { .. } => stack.push(ConstExprResult::I64(0)),
                Instruction::RefFunc { .. } => todo!(),
                Instruction::GlobalGet { global } => {
                    let global = self.globals[global].as_ref().unwrap();
                    // ensure!(
                    //     matches!(global.ty.mutability, parser::Mutability::Const),
                    //     Error::MutableGlobalInConst
                    // );
                    stack.push(self.eval_const_expr(&global.init)?);
                }
                _ => panic!("not a constant instruction"),
            }
        }

        // ensure!(stack.len() == 1, Error::ConstExpressionTooLong);

        Ok(stack.swap_remove(0))
    }
}

impl<'a, 'wasm> TargetEnvironment for FunctionEnvironment<'a, 'wasm> {
    fn target_config(&self) -> TargetFrontendConfig {
        self.target_isa.frontend_config()
    }

    fn proof_carrying_code(&self) -> bool {
        todo!()
    }
}

impl<'a, 'wasm> cranelift_wasm::FuncTranslationEnvironment for FunctionEnvironment<'a, 'wasm> {
    fn lookup_type(&self, type_idx: wasmparser::TypeIdx) -> &wasmparser::FunctionType {
        self.types[type_idx].as_ref().unwrap()
    }

    fn make_global(
        &mut self,
        mut pos: FuncCursor,
        idx: wasmparser::GlobalIdx,
    ) -> Result<GlobalVariable, cranelift_wasm::Error> {
        let global = self.globals[idx].as_ref().unwrap();

        if global.ty.mutability == Mutability::Const {
            let val = match self.eval_const_expr(&global.init)? {
                ConstExprResult::I32(val) => pos.ins().iconst(types::I32, val as i64),
                ConstExprResult::I64(val) => pos.ins().iconst(types::I64, val),
                ConstExprResult::F32(val) => pos.ins().f32const(val),
                ConstExprResult::F64(val) => pos.ins().f64const(val),
            };

            Ok(GlobalVariable::Const(val))
        } else {
            let gv = self.get_globals_base();

            let ty = match global.ty.ty {
                ValueType::I32 => types::I32,
                ValueType::I64 => types::I64,
                ValueType::F32 => types::F32,
                ValueType::F64 => types::F64,
                ValueType::V128 => todo!(),
                ValueType::FuncRef | ValueType::ExternRef => todo!(),
            };

            Ok(GlobalVariable::Memory {
                gv,
                offset: Offset32::new((idx.as_bits() * 8) as i32),
                ty,
            })
        }
    }

    fn translate_global_get(
        &mut self,
        pos: FuncCursor,
        global_index: wasmparser::GlobalIdx,
    ) -> Result<ir::Value, cranelift_wasm::Error> {
        todo!("custom global get, this should not be called")
    }

    fn translate_global_set(
        &mut self,
        pos: FuncCursor,
        global_index: wasmparser::GlobalIdx,
        val: ir::Value,
    ) -> Result<(), cranelift_wasm::Error> {
        todo!("custom global set, this should not be called")
    }

    fn heaps(&self) -> &PrimaryMap<cranelift_wasm::Heap, cranelift_wasm::HeapData> {
        &self.heaps
    }

    fn make_heap(
        &mut self,
        func: &mut ir::Function,
        idx: wasmparser::MemIdx,
    ) -> Result<cranelift_wasm::Heap, cranelift_wasm::Error> {
        let memory = self.memories[idx].as_ref().unwrap();

        let (min_size, max_size) = match memory.limits {
            Limits::Unbounded(min) => (min as u64, self.memory_max_size),
            Limits::Bounded(min, max) => {
                (min as u64, core::cmp::min(max as u64, self.memory_max_size))
            }
        };

        let memory_type = func.create_memory_type(MemoryTypeData::Memory { size: max_size });

        Ok(self.heaps.push(HeapData {
            base: self.get_heap_base(),
            min_size,
            max_size,
            memory64: true,
            index_type: self.target_config().pointer_type(),
            memory_type: Some(memory_type),
        }))
    }

    fn translate_memory_init(
        &mut self,
        pos: FuncCursor,
        index: wasmparser::MemIdx,
        heap: cranelift_wasm::Heap,
        data_index: wasmparser::DataIdx,
        dst: ir::Value,
        src: ir::Value,
        len: ir::Value,
    ) -> Result<(), cranelift_wasm::Error> {
        todo!()
    }

    fn translate_memory_grow(
        &mut self,
        mut pos: FuncCursor,
        index: wasmparser::MemIdx,
        heap: cranelift_wasm::Heap,
        pages: ir::Value,
    ) -> Result<ir::Value, cranelift_wasm::Error> {
        todo!()
    }

    fn translate_memory_size(
        &mut self,
        pos: FuncCursor,
        index: wasmparser::MemIdx,
        heap: cranelift_wasm::Heap,
    ) -> Result<ir::Value, cranelift_wasm::Error> {
        todo!()
    }

    fn translate_memory_copy(
        &mut self,
        pos: FuncCursor,
        src_index: wasmparser::MemIdx,
        src_heap: cranelift_wasm::Heap,
        dst_index: wasmparser::MemIdx,
        dst_heap: cranelift_wasm::Heap,
        dst: ir::Value,
        src: ir::Value,
        len: ir::Value,
    ) -> Result<(), cranelift_wasm::Error> {
        todo!()
    }

    fn translate_memory_fill(
        &mut self,
        pos: FuncCursor,
        index: wasmparser::MemIdx,
        heap: cranelift_wasm::Heap,
        dst: ir::Value,
        val: ir::Value,
        len: ir::Value,
    ) -> Result<(), cranelift_wasm::Error> {
        todo!()
    }

    fn translate_data_drop(
        &mut self,
        pos: FuncCursor,
        data_index: wasmparser::DataIdx,
    ) -> Result<(), cranelift_wasm::Error> {
        todo!()
    }

    fn translate_memory_discard(
        &mut self,
        pos: FuncCursor,
        index: wasmparser::MemIdx,
        heap: cranelift_wasm::Heap,
    ) -> Result<(), cranelift_wasm::Error> {
        todo!()
    }

    fn make_table(
        &mut self,
        func: &mut ir::Function,
        idx: wasmparser::TableIdx,
    ) -> Result<ir::Table, cranelift_wasm::Error> {
        let table = self.tables[idx].as_ref().unwrap();

        let (min_size, max_size) = match table.limits {
            Limits::Unbounded(min) => (min as u64, wasmparser::MAX_WASM_TABLE_ENTRIES as u64),
            Limits::Bounded(min, max) => (
                min as u64,
                core::cmp::min(max as u64, wasmparser::MAX_WASM_TABLE_ENTRIES as u64),
            ),
        };

        let base_gv = func.create_global_value(GlobalValueData::IAddImm {
            base: self.get_tables_base(),
            offset: Imm64::new(self.target_config().pointer_bytes() as i64),
            global_type: self.target_config().pointer_type(),
        });

        let element_size = match table.ty {
            ReferenceType::FuncRef => self.target_config().pointer_bytes() as u64,
            ReferenceType::ExternRef => match self.target_config().pointer_type() {
                types::I32 => types::R32.bytes() as u64,
                types::I64 => types::R64.bytes() as u64,
                _ => panic!("unsupported pointer type"),
            },
        };

        Ok(func.tables.push(TableData {
            base_gv,
            min_size: Uimm64::new(min_size),
            bound_gv: self.get_tables_base(),
            element_size: Uimm64::new(element_size),
            index_type: Default::default(),
        }))
    }

    fn translate_table_init(
        &mut self,
        pos: FuncCursor,
        table_index: wasmparser::TableIdx,
        table: ir::Table,
        elem_index: wasmparser::ElemIdx,
        dst: ir::Value,
        src: ir::Value,
        len: ir::Value,
    ) -> Result<(), cranelift_wasm::Error> {
        todo!()
    }

    fn translate_table_grow(
        &mut self,
        pos: FuncCursor,
        table_index: wasmparser::TableIdx,
        table: ir::Table,
        delta: ir::Value,
        init_value: ir::Value,
    ) -> Result<ir::Value, cranelift_wasm::Error> {
        todo!()
    }

    fn translate_table_get(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: wasmparser::TableIdx,
        table: ir::Table,
        index: ir::Value,
    ) -> Result<ir::Value, cranelift_wasm::Error> {
        todo!()
    }

    fn translate_table_set(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: wasmparser::TableIdx,
        table: ir::Table,
        value: ir::Value,
        index: ir::Value,
    ) -> Result<(), cranelift_wasm::Error> {
        todo!()
    }

    fn translate_table_size(
        &mut self,
        pos: FuncCursor,
        index: wasmparser::TableIdx,
        table: ir::Table,
    ) -> Result<ir::Value, cranelift_wasm::Error> {
        todo!()
    }

    fn translate_table_copy(
        &mut self,
        pos: FuncCursor,
        dst_table_index: wasmparser::TableIdx,
        dst_table: ir::Table,
        src_table_index: wasmparser::TableIdx,
        src_table: ir::Table,
        dst: ir::Value,
        src: ir::Value,
        len: ir::Value,
    ) -> Result<(), cranelift_wasm::Error> {
        todo!()
    }

    fn translate_table_fill(
        &mut self,
        pos: FuncCursor,
        table_index: wasmparser::TableIdx,
        table: ir::Table,
        dst: ir::Value,
        val: ir::Value,
        len: ir::Value,
    ) -> Result<(), cranelift_wasm::Error> {
        todo!()
    }

    fn translate_elem_drop(
        &mut self,
        pos: FuncCursor,
        seg_index: wasmparser::ElemIdx,
    ) -> Result<(), cranelift_wasm::Error> {
        todo!()
    }

    fn make_indirect_sig(
        &mut self,
        func: &mut ir::Function,
        index: wasmparser::TypeIdx,
    ) -> Result<ir::SigRef, cranelift_wasm::Error> {
        todo!()
    }

    fn make_direct_func(
        &mut self,
        func: &mut ir::Function,
        index: wasmparser::FuncIdx,
    ) -> Result<ir::FuncRef, cranelift_wasm::Error> {
        todo!()
    }

    fn translate_call(
        &mut self,
        builder: &mut FunctionBuilder,
        callee_index: wasmparser::FuncIdx,
        callee: ir::FuncRef,
        call_args: &[ir::Value],
    ) -> Result<ir::Inst, cranelift_wasm::Error> {
        todo!()
    }

    fn translate_call_indirect(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: wasmparser::TableIdx,
        table: ir::Table,
        sig_index: wasmparser::TypeIdx,
        sig_ref: ir::SigRef,
        callee: ir::Value,
        call_args: &[ir::Value],
    ) -> Result<ir::Inst, cranelift_wasm::Error> {
        todo!()
    }

    fn translate_call_ref(
        &mut self,
        builder: &mut FunctionBuilder,
        sig_ref: ir::SigRef,
        callee: ir::Value,
        call_args: &[ir::Value],
    ) -> Result<ir::Inst, cranelift_wasm::Error> {
        todo!()
    }

    fn translate_return_call(
        &mut self,
        builder: &mut FunctionBuilder,
        callee_index: wasmparser::FuncIdx,
        callee: ir::FuncRef,
        call_args: &[ir::Value],
    ) -> Result<(), cranelift_wasm::Error> {
        todo!()
    }

    fn translate_return_call_indirect(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: wasmparser::TableIdx,
        table: ir::Table,
        sig_index: wasmparser::TypeIdx,
        sig_ref: ir::SigRef,
        callee: ir::Value,
        call_args: &[ir::Value],
    ) -> Result<(), cranelift_wasm::Error> {
        todo!()
    }

    fn translate_return_call_ref(
        &mut self,
        builder: &mut FunctionBuilder,
        sig_ref: ir::SigRef,
        callee: ir::Value,
        call_args: &[ir::Value],
    ) -> Result<(), cranelift_wasm::Error> {
        todo!()
    }
}

/// The memory layout of a process
///
/// struct MemoryLayout {
///     text: [u8],
///     globals: [usize],
///     tables: [Table],
///     memory: [u8]
/// }
///
/// struct Table {
///     len: u32,
///     elems: [??]
/// }

struct MemoryLayout {
    text: Range<usize>,
    globals: Range<usize>,
    tables: Range<usize>,
    memories: Range<usize>,
}
