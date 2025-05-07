// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![expect(unused, reason = "this module has a number of method stubs")]

use crate::wasm::compile::NS_WASM_FUNC;
use crate::wasm::cranelift::builtins::BuiltinFunctions;
use crate::wasm::cranelift::code_translator::Reachability;
use crate::wasm::cranelift::memory::CraneliftMemory;
use crate::wasm::cranelift::utils::index_type_to_ir_type;
use crate::wasm::cranelift::{CraneliftGlobal, CraneliftTable};
use crate::wasm::indices::{
    CanonicalizedTypeIndex, DataIndex, ElemIndex, FuncIndex, GlobalIndex, MemoryIndex, TableIndex,
    TypeIndex, VMSharedTypeIndex,
};
use crate::wasm::translate::{
    IndexType, Memory, ModuleTypes, Table, TranslatedModule, WasmHeapTopType, WasmHeapType,
    WasmHeapTypeInner, WasmRefType, WasmparserTypeConverter,
};
use crate::wasm::trap::{TRAP_BAD_SIGNATURE, TRAP_INDIRECT_CALL_TO_NULL};
use crate::wasm::utils::{
    reference_type, u8_size_of, u32_offset_of, value_type, wasm_call_signature,
};
use crate::wasm::vm::{
    StaticVMShape, VMFuncRef, VMFunctionImport, VMGlobalImport, VMMemoryDefinition, VMMemoryImport,
    VMShape, VMTableDefinition, VMTableImport,
};
use alloc::vec;
use alloc::vec::Vec;
use core::cmp;
use core::mem::offset_of;
use cranelift_codegen::cursor::FuncCursor;
use cranelift_codegen::ir;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::immediates::Offset32;
use cranelift_codegen::ir::types::{I32, I64};
use cranelift_codegen::ir::{
    ArgumentPurpose, ExtFuncData, ExternalName, FuncRef, GlobalValue, GlobalValueData, Inst,
    MemFlags, MemoryType, SigRef, Signature, TrapCode, Type, UserExternalName, Value,
};
use cranelift_codegen::ir::{Function, InstBuilder};
use cranelift_codegen::isa::TargetIsa;
use cranelift_frontend::FunctionBuilder;
use smallvec::SmallVec;

/// A smallvec that holds the IR values for a struct's fields.
pub type StructFieldsVec = SmallVec<[Value; 4]>;

#[expect(clippy::struct_excessive_bools, reason = "TODO replace with bitflags")]
pub struct TranslationEnvironment<'module_env> {
    isa: &'module_env dyn TargetIsa,
    module: &'module_env TranslatedModule,
    types: &'module_env ModuleTypes,
    vmshape: VMShape,

    /// Caches of signatures for builtin functions.
    builtin_functions: BuiltinFunctions,

    /// The Cranelift global holding the vmctx address.
    vmctx: Option<GlobalValue>,
    /// The PCC memory type describing the vmctx layout, if we're
    /// using PCC.
    pcc_vmctx_memtype: Option<MemoryType>,

    /// Whether to force relaxed simd instructions to be deterministic.
    relaxed_simd_deterministic: bool,
    /// Whether to use the heap access spectre mitigation.
    heap_access_spectre_mitigation: bool,
    table_access_spectre_mitigation: bool,
    /// Whether to use proof-carrying code to verify lowerings.
    proof_carrying_code: bool,
}

impl<'module_env> TranslationEnvironment<'module_env> {
    pub(crate) fn new(
        isa: &'module_env dyn TargetIsa,
        module: &'module_env TranslatedModule,
        types: &'module_env ModuleTypes,
    ) -> Self {
        let vmoffsets = VMShape::for_module(isa.pointer_bytes(), module);
        let builtin_functions = BuiltinFunctions::new(isa);
        Self {
            isa,
            module,
            types,
            vmshape: vmoffsets,
            builtin_functions,

            vmctx: None,
            pcc_vmctx_memtype: None,

            relaxed_simd_deterministic: false,
            heap_access_spectre_mitigation: true,
            table_access_spectre_mitigation: true,
            proof_carrying_code: true,
        }
    }

    fn vmctx(&mut self, func: &mut Function) -> GlobalValue {
        self.vmctx.unwrap_or_else(|| {
            let vmctx = func.create_global_value(GlobalValueData::VMContext);

            if self.isa.flags().enable_pcc() {
                // Create a placeholder memtype for the vmctx; we'll
                // add fields to it as we lazily create HeapData
                // structs and global values.
                let vmctx_memtype = func.create_memory_type(ir::MemoryTypeData::Struct {
                    size: 0,
                    fields: vec![],
                });

                self.pcc_vmctx_memtype = Some(vmctx_memtype);
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

    pub(crate) fn vmctx_val(&mut self, pos: &mut FuncCursor<'_>) -> Value {
        let pointer_type = self.pointer_type();
        let vmctx = self.vmctx(pos.func);
        pos.ins().global_value(pointer_type, vmctx)
    }

    fn get_global_location(
        &mut self,
        func: &mut Function,
        index: GlobalIndex,
    ) -> (GlobalValue, i32) {
        let vmctx = self.vmctx(func);
        if let Some(def_index) = self.module.defined_global_index(index) {
            let offset = i32::try_from(self.vmshape.vmctx_vmglobal_definition(def_index)).unwrap();
            (vmctx, offset)
        } else {
            let from_offset =
                self.vmshape.vmctx_vmglobal_import(index) + u32_offset_of!(VMGlobalImport, from);
            let global = func.create_global_value(GlobalValueData::Load {
                base: vmctx,
                offset: Offset32::new(i32::try_from(from_offset).unwrap()),
                global_type: self.pointer_type(),
                flags: MemFlags::trusted().with_readonly(),
            });
            (global, 0)
        }
    }

    /// Proof-carrying code: create a memtype describing an empty
    /// runtime struct (to be updated later).
    fn create_empty_struct_memtype(&self, func: &mut Function) -> MemoryType {
        func.create_memory_type(ir::MemoryTypeData::Struct {
            size: 0,
            fields: vec![],
        })
    }
    fn add_field_to_memtype(
        &self,
        func: &mut Function,
        memtype: MemoryType,
        offset: u32,
        pointee: MemoryType,
        readonly: bool,
    ) {
        let ptr_size = self.pointer_type().bytes();
        match &mut func.memory_types[memtype] {
            ir::MemoryTypeData::Struct { size, fields } => {
                *size = cmp::max(*size, u64::from(offset + ptr_size));
                fields.push(ir::MemoryTypeField {
                    ty: self.pointer_type(),
                    offset: offset.into(),
                    readonly,
                    fact: Some(ir::Fact::Mem {
                        ty: pointee,
                        min_offset: 0,
                        max_offset: 0,
                        nullable: false,
                    }),
                });

                // Sort fields by offset -- we need to do this now
                // because we may create an arbitrary number of
                // memtypes for imported memories and we don't
                // otherwise track them.
                fields.sort_by_key(|f| f.offset);
            }
            _ => panic!("Cannot add field to non-struct memtype"),
        }
    }
    /// Generate a load that loads a pointer from the given address. If using pcc will add
    /// a field to memype struct and  a new memtype for the pointee.
    fn load_pointer_with_memtypes(
        &self,
        func: &mut Function,
        value: GlobalValue,
        offset: u32,
        readonly: bool,
        memtype: Option<MemoryType>,
    ) -> (GlobalValue, Option<MemoryType>) {
        let pointee = func.create_global_value(ir::GlobalValueData::Load {
            base: value,
            offset: Offset32::new(i32::try_from(offset).unwrap()),
            global_type: self.pointer_type(),
            flags: MemFlags::trusted().with_readonly(),
        });

        let mt = memtype.map(|mt| {
            let pointee_mt = self.create_empty_struct_memtype(func);
            self.add_field_to_memtype(func, mt, offset, pointee_mt, readonly);
            func.global_value_facts[pointee] = Some(ir::Fact::Mem {
                ty: pointee_mt,
                min_offset: 0,
                max_offset: 0,
                nullable: false,
            });
            pointee_mt
        });
        (pointee, mt)
    }

    fn memory(&self, memory_index: MemoryIndex) -> &Memory {
        &self.module.memories[memory_index]
    }

    fn table(&self, table_index: TableIndex) -> &Table {
        &self.module.tables[table_index]
    }

    /// Cast the value to I64 and sign extend if necessary.
    ///
    /// Returns the value casted to I64.
    fn cast_index_to_i64(
        &self,
        pos: &mut FuncCursor<'_>,
        val: Value,
        index_type: IndexType,
    ) -> Value {
        match index_type {
            IndexType::I32 => pos.ins().uextend(I64, val),
            IndexType::I64 => val,
        }
    }

    /// Convert the target pointer-sized integer `val` into the memory/table's index type.
    ///
    /// For memory, `val` is holding a memory length (or the `-1` `memory.grow`-failed sentinel).
    /// For table, `val` is holding a table length.
    ///
    /// This might involve extending or truncating it depending on the memory/table's
    /// index type and the target's pointer type.
    fn convert_pointer_to_index_type(
        &self,
        mut pos: FuncCursor<'_>,
        val: Value,
        index_type: IndexType,
        // When it is a memory and the memory is using single-byte pages,
        // we need to handle the tuncation differently. See comments below.
        //
        // When it is a table, this should be set to false.
        single_byte_pages: bool,
    ) -> Value {
        let desired_type = index_type_to_ir_type(index_type);
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
            // We have a 64-bit memory/table on a 32-bit host -- this combo doesn't
            // really make a whole lot of sense to do from a user perspective
            // but that is neither here nor there. We want to logically do an
            // unsigned extend *except* when we are given the `-1` sentinel,
            // which we must preserve as `-1` in the wider type.
            if single_byte_pages {
                // For single-byte pages, we have to explicitly check for
                // `-1` and choose whether to do an unsigned extension or
                // return a larger `-1` because there are valid memory
                // lengths (in pages) that have the sign bit set.
                let extended = pos.ins().uextend(desired_type, val);
                let neg_one = pos.ins().iconst(desired_type, -1);
                let is_failure = pos.ins().icmp_imm(IntCC::Equal, val, -1);
                pos.ins().select(is_failure, neg_one, extended)
            } else {
                // In the case that we have default page sizes, we can
                // always sign extend, since valid memory lengths (in pages)
                // never have their sign bit set, and so if the sign bit is
                // set then this must be the `-1` sentinel, which we want to
                // preserve through the extension.
                //
                // When it comes to table, `single_byte_pages` should have always been set to false.
                // Then we simply do a signed extension.
                pos.ins().sextend(desired_type, val)
            }
        }
    }
}

impl TranslationEnvironment<'_> {
    pub fn make_direct_func(&self, func: &mut Function, index: FuncIndex) -> FuncRef {
        let sig_index = self.module.functions[index]
            .signature
            .unwrap_module_type_index();
        let sig = self.types.get_wasm_type(sig_index).unwrap().unwrap_func();

        let signature = func.import_signature(wasm_call_signature(self.isa, sig));
        let name =
            ExternalName::User(func.declare_imported_user_function(UserExternalName::new(
                NS_WASM_FUNC,
                index.as_u32(),
            )));

        func.import_function(ExtFuncData {
            name,
            signature,
            colocated: self.module.defined_func_index(index).is_some(),
        })
    }

    pub fn make_indirect_sig(&self, func: &mut Function, sig_index: TypeIndex) -> SigRef {
        let interned_index = self.module.types[sig_index];
        let wasm_func_ty = self
            .types
            .get_wasm_type(interned_index)
            .unwrap()
            .unwrap_func();
        let sig = wasm_call_signature(self.isa, wasm_func_ty);
        func.import_signature(sig)
    }

    #[expect(clippy::cast_possible_wrap, reason = "this is fiiinee")]
    pub fn make_table(&mut self, func: &mut Function, index: TableIndex) -> CraneliftTable {
        let table = &self.module.tables[index];
        let vmctx = self.vmctx(func);
        let pointer_type = self.pointer_type();

        let (base, base_offset) = if let Some(def_index) = self.module.defined_table_index(index) {
            let base_offset = self.vmshape.vmctx_vmtable_definition(def_index)
                + u32_offset_of!(VMTableDefinition, base);

            (vmctx, base_offset)
        } else {
            let from_offset =
                self.vmshape.vmctx_vmtable_import(index) + u32_offset_of!(VMTableImport, from);
            let table = func.create_global_value(ir::GlobalValueData::Load {
                base: vmctx,
                offset: Offset32::new(i32::try_from(from_offset).unwrap()),
                global_type: pointer_type,
                flags: MemFlags::trusted().with_readonly(),
            });
            let base_offset = u32_offset_of!(VMTableDefinition, base);

            (table, base_offset)
        };

        let table_base = func.create_global_value(GlobalValueData::Load {
            base,
            offset: Offset32::from(base_offset as i32),
            global_type: pointer_type,
            flags: MemFlags::trusted().with_checked().with_readonly(),
        });

        let element_size = if table.element_type.is_vmgcref_type() {
            // For GC-managed references, tables store `Option<VMGcRef>`s.
            I32.bytes()
        } else {
            self.reference_type(&table.element_type.heap_type).0.bytes()
        };

        let bound = if Some(table.limits.min) == table.limits.max {
            table.limits.min
        } else {
            todo!("resizable tables")
        };

        CraneliftTable {
            base_gv: table_base,
            bound,
            element_size,
        }
    }

    #[expect(clippy::cast_possible_wrap, reason = "this is fiiinee")]
    pub fn make_memory(&mut self, func: &mut Function, index: MemoryIndex) -> CraneliftMemory {
        let plan = &self.module.memories[index];
        let vmctx = self.vmctx(func);

        let (base, base_offset, ptr_memtype) = match self.module.defined_memory_index(index) {
            Some(_) if plan.shared => todo!("shared memory"),
            Some(def_index) => {
                let owned_index = self.module.owned_memory_index(def_index);
                let base_offset = self.vmshape.vmctx_vmmemory_definition(owned_index)
                    + u32_offset_of!(VMMemoryDefinition, base);

                (vmctx, base_offset, self.pcc_vmctx_memtype)
            }
            None => {
                let from_offset = self.vmshape.vmctx_vmmemory_import(index)
                    + u32_offset_of!(VMMemoryImport, from);

                // load the pointer to the memory from our VMMemoryImport
                let (memory, def_mt) = self.load_pointer_with_memtypes(
                    func,
                    vmctx,
                    from_offset,
                    true,
                    self.pcc_vmctx_memtype,
                );
                let base_offset = u32_offset_of!(VMMemoryDefinition, base);
                (memory, base_offset, def_mt)
            }
        };

        let (base_fact, memory_type) = if let Some(ptr_memtype) = ptr_memtype {
            // Create a memtype representing the untyped memory region.
            let data_mt = func.create_memory_type(ir::MemoryTypeData::Memory {
                // Since we have one memory per address space, the maximum value this can be is u64::MAX
                // TODO this isn't correct I think
                size: plan.max_size_based_on_index_type(),
            });
            // This fact applies to any pointer to the start of the memory.
            let base_fact = ir::Fact::Mem {
                ty: data_mt,
                min_offset: 0,
                max_offset: 0,
                nullable: false,
            };
            // Create a field in the vmctx for the base pointer.
            match &mut func.memory_types[ptr_memtype] {
                ir::MemoryTypeData::Struct { size, fields } => {
                    let offset = u64::from(base_offset);
                    fields.push(ir::MemoryTypeField {
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
                    *size = cmp::max(
                        *size,
                        offset.saturating_add(u64::from(self.isa.pointer_type().bytes())),
                    );
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

        let heap_base = func.create_global_value(GlobalValueData::Load {
            base,
            offset: Offset32::new(base_offset as i32),
            global_type: self.pointer_type(),
            flags: MemFlags::trusted().with_checked().with_readonly(),
        });
        func.global_value_facts[heap_base] = base_fact;

        let min_size = plan.minimum_byte_size().unwrap_or_else(|_| {
            // The only valid Wasm memory size that won't fit in a 64-bit
            // integer is the maximum memory64 size (2^64) which is one
            // larger than `u64::MAX` (2^64 - 1). In this case, just say the
            // minimum heap size is `u64::MAX`.
            debug_assert_eq!(plan.limits.min, 1 << 48i32);
            debug_assert_eq!(plan.page_size(), 1 << 16i32);
            u64::MAX
        });
        let max_size = plan.maximum_byte_size().ok();

        CraneliftMemory {
            base_gv: heap_base,
            memory_type,
            min_size,
            max_size,
            bound: plan.max_size_based_on_index_type(),
            index_type: plan.index_type,
            offset_guard_size: plan.offset_guard_size,
            page_size_log2: plan.page_size_log2,
        }
    }

    pub(crate) fn make_global(
        &mut self,
        func: &mut Function,
        index: GlobalIndex,
    ) -> CraneliftGlobal {
        let global = &self.module.globals[index];
        debug_assert!(!global.shared);

        let (gv, offset) = self.get_global_location(func, index);

        CraneliftGlobal::Memory {
            gv,
            offset: offset.into(),
            ty: value_type(
                &self.module.globals[index].content_type,
                self.pointer_type(),
            ),
        }
    }
    pub fn target_isa(&self) -> &dyn TargetIsa {
        self.isa
    }
    /// Whether or not to force relaxed simd instructions to have deterministic
    /// lowerings meaning they will produce the same results across all hosts,
    /// regardless of the cost to performance.
    pub fn relaxed_simd_deterministic(&self) -> bool {
        self.relaxed_simd_deterministic
    }
    pub fn heap_access_spectre_mitigation(&self) -> bool {
        self.heap_access_spectre_mitigation
    }
    pub fn table_access_spectre_mitigation(&self) -> bool {
        self.table_access_spectre_mitigation
    }
    pub fn proof_carrying_code(&self) -> bool {
        self.proof_carrying_code
    }

    /// Get the Cranelift integer type to use for native pointers.
    ///
    /// This returns `I64` for 64-bit architectures and `I32` for 32-bit architectures.
    pub fn pointer_type(&self) -> Type {
        self.target_isa().pointer_type()
    }

    /// Get the Cranelift reference type to use for the given Wasm reference
    /// type.
    ///
    /// Returns a pair of the CLIF reference type to use and a boolean that
    /// describes whether the value should be included in GC stack maps or not.
    pub fn reference_type(&self, hty: &WasmHeapType) -> (Type, bool) {
        let ty = reference_type(hty, self.pointer_type());
        let needs_stack_map = match hty.top().0 {
            WasmHeapTopType::Extern | WasmHeapTopType::Any => true,
            WasmHeapTopType::Func => false,
            _ => todo!(),
        };
        (ty, needs_stack_map)
    }

    pub(crate) fn convert_heap_type(&self, ty: wasmparser::HeapType) -> WasmHeapType {
        WasmparserTypeConverter::new(self.types, self.module).convert_heap_type(ty)
    }

    pub fn has_native_fma(&self) -> bool {
        self.target_isa().has_native_fma()
    }
    pub fn is_x86(&self) -> bool {
        self.target_isa().triple().architecture == target_lexicon::Architecture::X86_64
    }
    pub fn use_x86_blendv_for_relaxed_laneselect(&self, ty: Type) -> bool {
        self.target_isa().has_x86_blendv_lowering(ty)
    }
    pub fn use_x86_pshufb_for_relaxed_swizzle(&self) -> bool {
        self.target_isa().has_x86_pshufb_lowering()
    }
    pub fn use_x86_pmulhrsw_for_relaxed_q15mul(&self) -> bool {
        self.target_isa().has_x86_pmulhrsw_lowering()
    }
    pub fn use_x86_pmaddubsw_for_dot(&self) -> bool {
        self.target_isa().has_x86_pmaddubsw_lowering()
    }

    /// Is the given parameter of the given function a wasm parameter or
    /// an internal implementation-detail parameter?
    pub fn is_wasm_parameter(&self, index: usize) -> bool {
        // The first two parameters are the function vmctx and caller vmctx. The rest are
        // the wasm parameters.
        index >= 2
    }

    /// Is the given parameter of the given function a wasm parameter or
    /// an internal implementation-detail parameter?
    pub fn is_wasm_return(&self, signature: &Signature, index: usize) -> bool {
        signature.returns[index].purpose == ir::ArgumentPurpose::Normal
    }

    /// Is the given parameter a GC reference and needs to be included in the stack map?
    pub fn func_ref_result_needs_stack_map(
        &self,
        func: &mut Function,
        func_ref: FuncRef,
        index: usize,
    ) -> bool {
        // TODO stack map
        false
    }

    /// Is the given result a GC reference and needs to be included in the stack map?
    pub fn sig_ref_result_needs_stack_map(&self, sig_ref: SigRef, index: usize) -> bool {
        // TODO stack map
        false
    }

    /// Translate a WASM `global.get` instruction at the builder's current position
    /// for a global that is custom.
    pub fn translate_custom_global_get(
        &mut self,
        builder: &mut FunctionBuilder,
        index: GlobalIndex,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a WASM `global.set` instruction at the builder's current position
    /// for a global that is custom.
    pub fn translate_custom_global_set(
        &mut self,
        builder: &mut FunctionBuilder,
        index: GlobalIndex,
        value: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `call` instruction at the builder's current
    /// position.
    ///
    /// Insert instructions for a *direct call* to the function `callee_index`.
    /// The function reference `callee` was previously created by `make_direct_func()`.
    ///
    /// Return the call instruction whose results are the WebAssembly return values.
    pub fn translate_call(
        &mut self,
        builder: &mut FunctionBuilder,
        callee_index: FuncIndex,
        callee: FuncRef,
        call_args: &[Value],
    ) -> Inst {
        CallBuilder::new(builder, self).direct_call(callee_index, callee, call_args)
    }

    /// Translate a WASM `call_indirect` instruction at the builder's current
    /// position.
    ///
    /// Insert instructions for an *indirect call* to the function `callee` in the table
    /// `table_index` with WASM signature `sig_index`. The `callee` value will have type
    /// `i32`.
    /// The signature `sig_ref` was previously created by `make_indirect_sig()`.
    ///
    /// Return the call instruction whose results are the WebAssembly return values.
    /// Returns `None` if this statically trap_handling instead of creating a call
    /// instruction.
    #[expect(clippy::too_many_arguments, reason = "")]
    pub fn translate_call_indirect(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: TableIndex,
        table: &CraneliftTable,
        sig_index: TypeIndex,
        sig_ref: SigRef,
        callee: Value,
        args: &[Value],
    ) -> Reachability<Inst> {
        CallBuilder::new(builder, self).indirect_call(
            table_index,
            table,
            sig_index,
            sig_ref,
            callee,
            args,
        )
    }

    /// Translate a WASM `call_ref` instruction at the builder's current
    /// position.
    ///
    /// Insert instructions at the builder's current position for an *indirect call*
    /// to the function `callee`. The `callee` value will be a Wasm funcref
    /// that needs to be translated to a native function address.
    ///
    /// `may_be_null` indicates whether a null check is necessary and is only false when
    /// we can statically prove through validation that the funcref can never be null.
    ///
    /// The signature `sig_ref` was previously created by `make_indirect_sig()`.
    ///
    /// Return the call instruction whose results are the WebAssembly return values.
    pub fn translate_call_ref(
        &mut self,
        builder: &mut FunctionBuilder,
        sig_ref: SigRef,
        callee: Value,
        args: &[Value],
        may_be_null: bool,
    ) -> crate::Result<Inst> {
        todo!()
    }

    /// Translate a WASM `return_call` instruction at the builder's
    /// current position.
    ///
    /// Insert instructions at the builder's current position for a *direct tail call*
    /// to the function `callee_index`.
    ///
    /// The function reference `callee` was previously created by `make_direct_func()`.
    ///
    /// Return the call instruction whose results are the WebAssembly return values.
    pub fn translate_return_call(
        &mut self,
        builder: &mut FunctionBuilder,
        callee_index: FuncIndex,
        callee: FuncRef,
        args: &[Value],
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `return_call_indirect` instruction at the
    /// builder's current position.
    ///
    /// Insert instructions at the builder's current position for an *indirect tail call*
    /// to the function `callee` in the table `table_index` with WebAssembly signature
    /// `sig_index`. The `callee` value will have type `i32`.
    ///
    /// The signature `sig_ref` was previously created by `make_indirect_sig()`.
    pub fn translate_return_call_indirect(
        &mut self,
        builder: &mut FunctionBuilder,
        table_index: TableIndex,
        type_index: TypeIndex,
        sig_ref: SigRef,
        callee: Value,
        args: &[Value],
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `return_call_ref` instruction at the builder's
    /// current position.
    ///
    /// Insert instructions at the builder's current position for an *indirect tail call*
    /// to the function `callee`. The `callee` value will be a Wasm funcref that may need
    /// to be translated to a native function address depending on your implementation of
    /// this trait.
    ///
    /// The signature `sig_ref` was previously created by `make_indirect_sig()`.
    pub fn translate_return_call_ref(
        &mut self,
        builder: &mut FunctionBuilder,
        sig_ref: SigRef,
        callee: Value,
        args: &[Value],
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `memory.grow` instruction at `pos`.
    ///
    /// The `memory_index` identifies the linear memory to grow and `delta` is the
    /// requested memory size in WASM pages.
    ///
    /// Returns the old size (in WASM pages) of the memory or `-1` to indicate failure.
    pub fn translate_memory_grow(
        &mut self,
        mut pos: FuncCursor,
        memory_index: MemoryIndex,
        delta: Value,
    ) -> Value {
        let memory_grow = self.builtin_functions.memory_grow(pos.func);

        let vmctx = self.vmctx_val(&mut pos);
        let memory_index_arg = pos.ins().iconst(I32, i64::from(memory_index.as_u32()));
        let delta = self.cast_index_to_i64(&mut pos, delta, self.memory(memory_index).index_type);

        let call_inst = pos
            .ins()
            .call(memory_grow, &[vmctx, memory_index_arg, delta]);
        let result = *pos.func.dfg.inst_results(call_inst).first().unwrap();

        let single_byte_pages = match self.memory(memory_index).page_size_log2 {
            16 => false,
            0 => true,
            _ => unreachable!("only page sizes 2**0 and 2**16 are currently valid"),
        };
        self.convert_pointer_to_index_type(
            pos,
            result,
            self.memory(memory_index).index_type,
            single_byte_pages,
        )
    }

    /// Translate a WASM `memory.size` instruction at `pos`.
    ///
    /// The `memory_index` identifies the linear memory.
    ///
    /// Returns the current size (in WASM pages) of the memory.
    #[expect(clippy::cast_possible_wrap, reason = "this is fiiinee")]
    pub fn translate_memory_size(
        &mut self,
        mut pos: FuncCursor,
        memory_index: MemoryIndex,
    ) -> crate::Result<Value> {
        let pointer_type = self.pointer_type();
        let vmctx = self.vmctx(pos.func);
        let is_shared = self.module.memories[memory_index].shared;
        let base = pos.ins().global_value(self.pointer_type(), vmctx);
        let current_length_in_bytes = match self.module.defined_memory_index(memory_index) {
            Some(def_index) => {
                if is_shared {
                    let offset = i32::try_from(self.vmshape.vmctx_vmmemory_pointer(def_index))?;
                    let vmmemory_ptr =
                        pos.ins()
                            .load(pointer_type, MemFlags::trusted(), base, offset);
                    let vmmemory_definition_offset =
                        i64::from(u32_offset_of!(VMMemoryDefinition, current_length));
                    let vmmemory_definition_ptr =
                        pos.ins().iadd_imm(vmmemory_ptr, vmmemory_definition_offset);
                    // This atomic access of the
                    // `VMMemoryDefinition::current_length` is direct; no bounds
                    // check is needed. This is possible because shared memory
                    // has a static size (the maximum is always known). Shared
                    // memory is thus built with a static memory plan and no
                    // bounds-checked version of this is implemented.
                    pos.ins().atomic_load(
                        pointer_type,
                        ir::MemFlags::trusted(),
                        vmmemory_definition_ptr,
                    )
                } else {
                    let owned_index = self.module.owned_memory_index(def_index);
                    let offset = i32::try_from(
                        self.vmshape.vmctx_vmmemory_definition(owned_index)
                            + u32_offset_of!(VMMemoryDefinition, current_length),
                    )?;
                    pos.ins()
                        .load(pointer_type, ir::MemFlags::trusted(), base, offset)
                }
            }
            None => {
                let offset = i32::try_from(
                    self.vmshape.vmctx_vmmemory_import(memory_index)
                        + u32_offset_of!(VMMemoryImport, from),
                )?;
                let vmmemory_ptr = pos
                    .ins()
                    .load(pointer_type, MemFlags::trusted(), base, offset);

                if is_shared {
                    let vmmemory_definition_offset =
                        i64::from(u32_offset_of!(VMMemoryDefinition, current_length));
                    let vmmemory_definition_ptr =
                        pos.ins().iadd_imm(vmmemory_ptr, vmmemory_definition_offset);
                    pos.ins().atomic_load(
                        pointer_type,
                        MemFlags::trusted(),
                        vmmemory_definition_ptr,
                    )
                } else {
                    pos.ins().load(
                        pointer_type,
                        MemFlags::trusted(),
                        vmmemory_ptr,
                        u32_offset_of!(VMMemoryDefinition, current_length) as i32,
                    )
                }
            }
        };
        let page_size_log2 = i64::from(self.module.memories[memory_index].page_size_log2);
        let current_length_in_pages = pos.ins().ushr_imm(current_length_in_bytes, page_size_log2);

        let single_byte_pages = match self.memory(memory_index).page_size_log2 {
            16 => false,
            0 => true,
            _ => unreachable!("only page sizes 2**0 and 2**16 are currently valid"),
        };
        Ok(self.convert_pointer_to_index_type(
            pos,
            current_length_in_pages,
            self.memory(memory_index).index_type,
            single_byte_pages,
        ))
    }

    /// Translate a WASM `memory.copy` instruction.
    ///
    /// The `src_index` and `dst_index` identify the source and destination linear memories respectively,
    /// `src_pos` and `dst_pos` are the source and destination offsets in bytes, and `len` is the number of bytes to copy.
    pub fn translate_memory_copy(
        &mut self,
        mut pos: FuncCursor,
        src_index: MemoryIndex,
        dst_index: MemoryIndex,
        src_pos: Value,
        dst_pos: Value,
        len: Value,
    ) {
        let memory_copy = self.builtin_functions.memory_copy(pos.func);

        let vmctx = self.vmctx_val(&mut pos);

        let dst = self.cast_index_to_i64(&mut pos, dst_pos, self.memory(dst_index).index_type);
        let src = self.cast_index_to_i64(&mut pos, src_pos, self.memory(src_index).index_type);

        // The length is 32-bit if either memory is 32-bit, but if they're both
        // 64-bit then it's 64-bit. Our intrinsic takes a 64-bit length for
        // compatibility across all memories, so make sure that it's cast
        // correctly here (this is a bit special so no generic helper unlike for
        // `dst`/`src` above)
        let len = if index_type_to_ir_type(self.memory(dst_index).index_type) == I64
            && index_type_to_ir_type(self.memory(src_index).index_type) == I64
        {
            len
        } else {
            pos.ins().uextend(I64, len)
        };

        let src_index = pos.ins().iconst(I32, i64::from(src_index.as_u32()));
        let dst_index = pos.ins().iconst(I32, i64::from(dst_index.as_u32()));

        pos.ins()
            .call(memory_copy, &[vmctx, dst_index, dst, src_index, src, len]);
    }

    /// Translate a WASM `memory.fill` instruction.
    ///
    /// The `memory_index` identifies the linear memory, `dst` is the offset in bytes, `val` is the
    /// value to fill the memory with and `len` is the number of bytes to fill.
    pub fn translate_memory_fill(
        &mut self,
        mut pos: FuncCursor,
        memory_index: MemoryIndex,
        dst: Value,
        value: Value,
        len: Value,
    ) {
        let memory_copy = self.builtin_functions.memory_fill(pos.func);

        let vmctx = self.vmctx_val(&mut pos);
        let dst = self.cast_index_to_i64(&mut pos, dst, self.memory(memory_index).index_type);
        let len = self.cast_index_to_i64(&mut pos, len, self.memory(memory_index).index_type);
        let memory_index = pos.ins().iconst(I32, i64::from(memory_index.as_u32()));

        pos.ins()
            .call(memory_copy, &[vmctx, memory_index, dst, value, len]);
    }

    /// Translate a WASM `memory.init` instruction.
    ///
    /// The `memory_index` identifies the linear memory amd `data_index` identifies the passive data segment.
    /// The `dst` value is the destination offset into the linear memory, `_src` is the offset into the
    /// data segment and `len` is the number of bytes to copy.
    pub fn translate_memory_init(
        &mut self,
        mut pos: FuncCursor,
        memory_index: MemoryIndex,
        data_index: DataIndex,
        dst: Value,
        src: Value,
        len: Value,
    ) {
        let memory_copy = self.builtin_functions.memory_init(pos.func);

        let vmctx = self.vmctx_val(&mut pos);
        let dst = self.cast_index_to_i64(&mut pos, dst, self.memory(memory_index).index_type);
        let len = self.cast_index_to_i64(&mut pos, len, self.memory(memory_index).index_type);
        let memory_index = pos.ins().iconst(I32, i64::from(memory_index.as_u32()));
        let data_index = pos.ins().iconst(I32, i64::from(data_index.as_u32()));

        pos.ins().call(
            memory_copy,
            &[vmctx, memory_index, data_index, dst, src, len],
        );
    }

    /// Translate a WASM `data.drop` instruction.
    pub fn translate_data_drop(&mut self, mut pos: FuncCursor, data_index: DataIndex) {
        let data_drop = self.builtin_functions.data_drop(pos.func);

        let vmctx = self.vmctx_val(&mut pos);
        let data_index = pos.ins().iconst(I32, i64::from(data_index.as_u32()));

        pos.ins().call(data_drop, &[vmctx, data_index]);
    }

    /// Translate a WASM `table.size` instruction.
    ///
    /// The `table_index` identifies the table.
    ///
    /// Returns the table size in elements.
    pub fn translate_table_size(
        &mut self,
        pos: FuncCursor,
        table_index: TableIndex,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a WASM `table.grow` instruction.
    ///
    /// The `table_index` identifies the table, `delta` is the number of elements to grow by
    /// and `initial_value` the value to fill the newly created elements with.
    ///
    /// Returns the old size of the table or `-1` to indicate failure.
    pub fn translate_table_grow(
        &mut self,
        pos: FuncCursor,
        table_index: TableIndex,
        delta: Value,
        initial_value: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a WASM `table.get` instruction.
    ///
    /// The `table_index` identifies the table and `index` is the index of the element to retrieve.
    ///
    /// Returns the element at the given index.
    pub fn translate_table_get(
        &mut self,
        pos: FuncCursor,
        table_index: TableIndex,
        index: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a WASM `table.set` instruction.
    ///
    /// The `table_index` identifies the table, `value` is the value to set and `index` is the index of the element to set.
    pub fn translate_table_set(
        &mut self,
        pos: FuncCursor,
        table_index: TableIndex,
        value: Value,
        index: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `table.copy` instruction.
    ///
    /// The `src_index` and `dst_index` identify the source and destination tables respectively,
    /// `dst` and `_src` are the destination and source offsets and `len` is the number of elements to copy.
    pub fn translate_table_copy(
        &mut self,
        pos: FuncCursor,
        src_index: TableIndex,
        dst_index: TableIndex,
        dst: Value,
        src: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `table.fill` instruction.
    ///
    /// The `table_index` identifies the table, `dst` is the offset, `value` is the value to fill the range.
    pub fn translate_table_fill(
        &mut self,
        pos: FuncCursor,
        table_index: TableIndex,
        dst: Value,
        value: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `table.init` instruction.
    ///
    /// The `table_index` identifies the table, `elem_index` identifies the passive element segment,
    /// `dst` is the destination offset, `_src` is the source offset and `len` is the number of elements to copy.
    pub fn translate_table_init(
        &mut self,
        pos: FuncCursor,
        table_index: TableIndex,
        elem_index: ElemIndex,
        dst: Value,
        src: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM `elem.drop` instruction.
    pub fn translate_elem_drop(
        &mut self,
        pos: FuncCursor,
        elem_index: ElemIndex,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a WASM i32.atomic.wait` or `i64.atomic.wait` instruction.
    ///
    /// The `memory_index` identifies the linear memory and `address` is the address to wait on.
    /// Whether the waited-on value is 32- or 64-bit can be determined by examining the type of
    /// `expected`, which must be only I32 or I64.
    ///
    /// TODO address?
    /// TODO timeout?
    /// TODO expected_value?
    ///
    /// Returns an i32, which is negative if the helper call failed.
    pub fn translate_atomic_wait(
        &mut self,
        pos: FuncCursor,
        memory_index: MemoryIndex,
        address: Value,
        expected_value: Value,
        timeout: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a WASM `atomic.notify` instruction.
    ///
    /// The `memory_index` identifies the linear memory.
    ///
    /// TODO address?
    /// TODO count?
    ///
    /// Returns an i64, which is negative if the helper call failed.
    pub fn translate_atomic_notify(
        &mut self,
        pos: FuncCursor,
        memory_index: MemoryIndex,
        address: Value,
        count: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a `ref.null T` WebAssembly instruction.
    pub fn translate_ref_null(&mut self, mut pos: FuncCursor, hty: &WasmHeapType) -> Value {
        assert!(!hty.shared);
        let (ty, _) = self.reference_type(hty);

        pos.ins().iconst(ty, 0)
    }

    /// Translate a `ref.is_null` WebAssembly instruction.
    pub fn translate_ref_is_null(&mut self, mut pos: FuncCursor, value: Value) -> Value {
        let byte_is_null = pos.ins().icmp_imm(IntCC::Equal, value, 0);
        pos.ins().uextend(I32, byte_is_null)
    }

    /// Translate a `ref.func` WebAssembly instruction.
    pub fn translate_ref_func(
        &mut self,
        pos: FuncCursor,
        index: FuncIndex,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `i32` value into an `i31ref`.
    pub fn translate_ref_i31(&mut self, pos: FuncCursor, value: Value) -> crate::Result<Value> {
        todo!()
    }

    /// Sign-extend an `i31ref` into an `i32`.
    pub fn translate_i31_get_s(&mut self, pos: FuncCursor, value: Value) -> crate::Result<Value> {
        todo!()
    }

    /// Zero-extend an `i31ref` into an `i32`.
    pub fn translate_i31_get_u(&mut self, pos: FuncCursor, value: Value) -> crate::Result<Value> {
        todo!()
    }
    // Translate a `struct.new` instruction.
    pub fn translate_struct_new(
        &mut self,
        builder: &mut FunctionBuilder,
        struct_type_index: TypeIndex,
        fields: StructFieldsVec,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a `struct.new_default` instruction.
    pub fn translate_struct_new_default(
        &mut self,
        builder: &mut FunctionBuilder,
        struct_type_index: TypeIndex,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a `struct.set` instruction.
    pub fn translate_struct_set(
        &mut self,
        builder: &mut FunctionBuilder,
        struct_type_index: TypeIndex,
        field_index: u32,
        struct_ref: Value,
        value: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a `struct.get` instruction.
    pub fn translate_struct_get(
        &mut self,
        builder: &mut FunctionBuilder,
        struct_type_index: TypeIndex,
        field_index: u32,
        struct_ref: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a `struct.get_s` instruction.
    pub fn translate_struct_get_s(
        &mut self,
        builder: &mut FunctionBuilder,
        struct_type_index: TypeIndex,
        field_index: u32,
        struct_ref: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate a `struct.get_u` instruction.
    pub fn translate_struct_get_u(
        &mut self,
        builder: &mut FunctionBuilder,
        struct_type_index: TypeIndex,
        field_index: u32,
        struct_ref: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.new` instruction.
    pub fn translate_array_new(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        elem: Value,
        len: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.new_default` instruction.
    pub fn translate_array_new_default(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        len: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.new_fixed` instruction.
    pub fn translate_array_new_fixed(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        elems: &[Value],
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.new_data` instruction.
    pub fn translate_array_new_data(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        data_index: DataIndex,
        data_offset: Value,
        len: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.new_elem` instruction.
    pub fn translate_array_new_elem(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        elem_index: ElemIndex,
        elem_offset: Value,
        len: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.copy` instruction.
    #[expect(clippy::too_many_arguments, reason = "")]
    pub fn translate_array_copy(
        &mut self,
        builder: &mut FunctionBuilder,
        dst_array_type_index: TypeIndex,
        dst_array: Value,
        dst_index: Value,
        src_array_type_index: TypeIndex,
        src_array: Value,
        src_index: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate an `array.fill` instruction.
    pub fn translate_array_fill(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        array: Value,
        index: Value,
        value: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate an `array.init_data` instruction.
    #[expect(clippy::too_many_arguments, reason = "")]
    pub fn translate_array_init_data(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        array: Value,
        dst_index: Value,
        data_index: DataIndex,
        data_offset: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate an `array.init_elem` instruction.
    #[expect(clippy::too_many_arguments, reason = "")]
    pub fn translate_array_init_elem(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        array: Value,
        dst_index: Value,
        elem_index: ElemIndex,
        elem_offset: Value,
        len: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate an `array.len` instruction.
    pub fn translate_array_len(
        &mut self,
        builder: &mut FunctionBuilder,
        array: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.get` instruction.
    pub fn translate_array_get(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        array: Value,
        index: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.get_s` instruction.
    pub fn translate_array_get_s(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        array: Value,
        index: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.get_u` instruction.
    pub fn translate_array_get_u(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        array: Value,
        index: Value,
    ) -> crate::Result<Value> {
        todo!()
    }

    /// Translate an `array.set` instruction.
    pub fn translate_array_set(
        &mut self,
        builder: &mut FunctionBuilder,
        array_type_index: TypeIndex,
        array: Value,
        index: Value,
        value: Value,
    ) -> crate::Result<()> {
        todo!()
    }

    /// Translate a `ref.test` instruction.
    pub fn translate_ref_test(
        &mut self,
        builder: &mut FunctionBuilder<'_>,
        ref_ty: WasmRefType,
        gc_ref: Value,
    ) -> crate::Result<Value> {
        todo!()
    }
}

pub(crate) struct CallBuilder<'a, 'func, 'module_env> {
    builder: &'a mut FunctionBuilder<'func>,
    env: &'a mut TranslationEnvironment<'module_env>,
    tail: bool,
}

enum CheckIndirectCallTypeSignature {
    Runtime,
    StaticMatch {
        /// Whether or not the funcref may be null or if it's statically known
        /// to not be null.
        may_be_null: bool,
    },
    /// The indirect call is statically known to trap.
    StaticTrap,
}

impl<'a, 'func, 'module_env> CallBuilder<'a, 'func, 'module_env> {
    /// Create a new `Call` site that will do regular, non-tail calls.
    pub fn new(
        builder: &'a mut FunctionBuilder<'func>,
        env: &'a mut TranslationEnvironment<'module_env>,
    ) -> Self {
        Self {
            builder,
            env,
            tail: false,
        }
    }

    /// Create a new `Call` site that will perform tail calls.
    pub fn new_tail(
        builder: &'a mut FunctionBuilder<'func>,
        env: &'a mut TranslationEnvironment<'module_env>,
    ) -> Self {
        Self {
            builder,
            env,
            tail: true,
        }
    }

    /// Call to a regular function with a statically known address used by `call` and `return_call`.
    ///
    /// Calls to locally-defined functions are translated into relocations while
    /// calls to imported functions do an indirect call through the `VMContext`s
    /// `imported_functions` array.
    pub fn direct_call(
        mut self,
        callee_index: FuncIndex,
        callee: FuncRef,
        call_args: &[Value],
    ) -> Inst {
        let mut real_call_args = Vec::with_capacity(call_args.len() + 2);
        let caller_vmctx = self
            .builder
            .func
            .special_param(ArgumentPurpose::VMContext)
            .unwrap();

        if !self.env.module.is_imported_func(callee_index) {
            // First append the callee vmctx address, which is the same as the caller vmctx in
            // this case.
            real_call_args.push(caller_vmctx);

            // Then append the caller vmctx address.
            real_call_args.push(caller_vmctx);

            // Then append the regular call arguments.
            real_call_args.extend_from_slice(call_args);

            // Finally, make the direct call!
            self.direct_call_inst(callee, &real_call_args)
        } else {
            // Handle direct calls to imported functions. We use an indirect call
            // so that we don't have to patch the code at runtime.
            let pointer_type = self.env.pointer_type();
            let sig_ref = self.builder.func.dfg.ext_funcs[callee].signature;
            let vmctx = self.env.vmctx(self.builder.func);
            let base = self.builder.ins().global_value(pointer_type, vmctx);

            let mem_flags = MemFlags::trusted().with_readonly();

            // Load the callee address.
            let body_offset = i32::try_from(
                self.env.vmshape.vmctx_vmfunction_import(callee_index)
                    + u32_offset_of!(VMFunctionImport, wasm_call),
            )
            .unwrap();
            let func_addr = self
                .builder
                .ins()
                .load(pointer_type, mem_flags, base, body_offset);

            // First append the callee vmctx address.
            let vmctx_offset = i32::try_from(
                self.env.vmshape.vmctx_vmfunction_import(callee_index)
                    + u32_offset_of!(VMFunctionImport, vmctx),
            )
            .unwrap();
            let vmctx = self
                .builder
                .ins()
                .load(pointer_type, mem_flags, base, vmctx_offset);
            real_call_args.push(vmctx);
            real_call_args.push(caller_vmctx);

            // Then append the regular call arguments.
            real_call_args.extend_from_slice(call_args);

            // Finally, make the indirect call!
            self.indirect_call_inst(sig_ref, func_addr, &real_call_args)
        }
    }

    /// Indirect call through the given funcref table used by [`call_indirect`][call_indirect] and
    /// [`return_call_indirect`][return_call_indirect].
    ///
    /// Indirect calls are translated to calls through the `VMContext` `func_ref` table, the spec
    /// requires us to check a few invariants:
    /// - That the table element `ty_index` exists (i.e. do a table bounds check).
    /// - That the table type matches the expected function type (indirect calls only allow funcref
    ///   table types).
    /// - That the element is non-null
    ///
    /// [call_indirect]: https://webassembly.github.io/spec/core/exec/instructions.html#xref-syntax-instructions-syntax-instr-control-mathsf-call-indirect-x-y
    /// [return_call_indirect]: https://webassembly.github.io/tail-call/core/exec/instructions.html#xref-syntax-instructions-syntax-instr-control-mathsf-return-call-indirect-x-y
    pub fn indirect_call(
        mut self,
        table_index: TableIndex,
        table: &CraneliftTable,
        ty_index: TypeIndex,
        sig_ref: SigRef,
        callee: Value,
        call_args: &[Value],
    ) -> Reachability<Inst> {
        let pointer_type = self.env.pointer_type();

        // Load the funcref pointer from the table.
        let (table_entry_addr, flags) = table.prepare_addr(
            self.builder,
            callee,
            pointer_type,
            self.env.table_access_spectre_mitigation(),
        );
        let funcref_ptr = self
            .builder
            .ins()
            .load(pointer_type, flags, table_entry_addr, 0i32);

        // If necessary, check the signature.
        let check = self.check_indirect_call_type_signature(table_index, ty_index, funcref_ptr);

        let trap_code = match check {
            // `funcref_ptr` is checked at runtime that its type matches,
            // meaning that if code gets this far it's guaranteed to not be
            // null. That means nothing in `unchecked_call` can fail.

            // `check_indirect_call_type_signature` has emitted a runtime type check
            // so nothing in `unchecked_indirect_call` can fail
            CheckIndirectCallTypeSignature::Runtime => None,
            // `funcref_ptr` is statically known to be the correct type, but it still might be null
            CheckIndirectCallTypeSignature::StaticMatch { may_be_null } => {
                may_be_null.then_some(TRAP_INDIRECT_CALL_TO_NULL)
            }
            // We statically know this will trap
            CheckIndirectCallTypeSignature::StaticTrap => return Reachability::Unreachable,
        };

        let (func_ptr, callee_vmctx) = self.load_func_and_vmctx(funcref_ptr, trap_code);

        let inst = self.unchecked_indirect_call(sig_ref, func_ptr, callee_vmctx, call_args);
        Reachability::Reachable(inst)
    }

    fn check_indirect_call_type_signature(
        &mut self,
        table_index: TableIndex,
        ty_index: TypeIndex,
        funcref_ptr: Value,
    ) -> CheckIndirectCallTypeSignature {
        let table = &self.env.module.tables[table_index];
        let sig_id_size = u8_size_of::<VMSharedTypeIndex>();
        let sig_id_type = Type::int(u16::from(sig_id_size).wrapping_mul(8)).unwrap();

        assert!(
            !table.element_type.heap_type.shared,
            "shared heap types not supported"
        );

        // The function references and GC proposals complicate the typecheck here somewhat
        // but essentially this all boils down to the "old" runtime signature check or a static
        // signature check for typed function references.
        let expected_type = &self.env.module.tables[table_index].element_type;
        match expected_type.heap_type.inner {
            // This is the old "funcref" (ref null func) type. This means inserting code
            // for a runtime signature check.
            WasmHeapTypeInner::Func => {
                let mem_flags = MemFlags::trusted().with_readonly();

                // load the expected type id from the `VMContext` `type_ids` array
                let expected_type_id = {
                    let vmctx = self.env.vmctx_val(&mut self.builder.cursor());
                    let type_ids = self.builder.ins().load(
                        self.env.pointer_type(),
                        mem_flags,
                        vmctx,
                        i32::from(StaticVMShape.vmctx_type_ids_array()),
                    );
                    let offset =
                        i32::try_from(ty_index.as_u32().checked_mul(sig_id_type.bytes()).unwrap())
                            .unwrap();
                    self.builder
                        .ins()
                        .load(sig_id_type, mem_flags, type_ids, offset)
                };

                // load the actual type id from the `VMFuncRef`
                let actual_type_id = self.builder.ins().load(
                    sig_id_type,
                    mem_flags.with_trap_code(Some(TRAP_INDIRECT_CALL_TO_NULL)),
                    funcref_ptr,
                    i32::try_from(offset_of!(VMFuncRef, type_index)).unwrap(),
                );

                // trap if they don't match
                let cmp = self
                    .builder
                    .ins()
                    .icmp(IntCC::Equal, expected_type_id, actual_type_id);
                self.builder.ins().trapz(cmp, TRAP_BAD_SIGNATURE);
                CheckIndirectCallTypeSignature::Runtime
            }
            // This is the typed function reference (ref $t) we can do a static signature check.
            WasmHeapTypeInner::ConcreteFunc(CanonicalizedTypeIndex::Module(expected_type)) => {
                // If the signatures match we don't need to emit any more checks
                let actual_ty = self.env.module.types[ty_index];
                if actual_ty == expected_type {
                    CheckIndirectCallTypeSignature::StaticMatch {
                        may_be_null: table.element_type.nullable,
                    }
                } else {
                    // Otherwise this is either a pointer with the wrong type *or* a null
                    // pointer. We need to trap in either case, but if the type is nullable
                    // we insert a null check first to produce the correct trap code.
                    if table.element_type.nullable {
                        // To check for a null pointer we just try to load its type index,
                        // if that fails because of a null pointer we fail with the correct code
                        // otherwise we fall through to the `TRAP_BAD_SIGNATURE` below.
                        let mem_flags = MemFlags::trusted().with_readonly();
                        self.builder.ins().load(
                            sig_id_type,
                            mem_flags.with_trap_code(Some(TRAP_INDIRECT_CALL_TO_NULL)),
                            funcref_ptr,
                            i32::try_from(offset_of!(VMFuncRef, type_index)).unwrap(),
                        );
                    }
                    self.builder.ins().trap(TRAP_BAD_SIGNATURE);
                    CheckIndirectCallTypeSignature::StaticTrap
                }
            }
            // This is the common subtype of all functions and can't be inhabited. So
            // this is always a trap.
            WasmHeapTypeInner::NoFunc => {
                assert!(table.element_type.nullable);
                self.builder.ins().trap(TRAP_INDIRECT_CALL_TO_NULL);
                CheckIndirectCallTypeSignature::StaticTrap
            }
            // We're dealing with un-canonicalized types at compilation stage, so finding `Shared`
            // or `RecGroup` indices here is a bug
            WasmHeapTypeInner::ConcreteFunc(
                CanonicalizedTypeIndex::Engine(_) | CanonicalizedTypeIndex::RecGroup(_),
            ) => {
                unreachable!(
                    "encountered shared or rec group indices during compilation. this is a bug"
                )
            }
            // All other heap types (GC types, exceptions, continuations) can't be called and won't
            // make it past validation
            _ => unreachable!(),
        }
    }

    /// Loads the function address and vmctx from the given `callee` value.
    /// `callee` has to be a function reference (i.e. a pointer into `VMContext`s `func_refs` array).
    fn load_func_and_vmctx(
        &mut self,
        callee: Value,
        callee_load_trap_code: Option<TrapCode>,
    ) -> (Value, Value) {
        let pointer_type = self.env.pointer_type();
        // Dereference callee pointer (pointer to a `VMFuncRef`) to get the function address.
        //
        // Note that this trap if `callee` is null, and it is the callers responsibility to
        // check whether `callee` is either already known to non-null or ay trap.
        // Therefore the `Option<TrapCode>`.
        let mem_flags = MemFlags::trusted().with_readonly();
        let func_addr = self.builder.ins().load(
            pointer_type,
            mem_flags.with_trap_code(callee_load_trap_code),
            callee,
            i32::try_from(offset_of!(VMFuncRef, wasm_call)).unwrap(),
        );
        let callee_vmctx = self.builder.ins().load(
            pointer_type,
            mem_flags,
            callee,
            i32::try_from(offset_of!(VMFuncRef, vmctx)).unwrap(),
        );

        (func_addr, callee_vmctx)
    }

    fn unchecked_indirect_call(
        mut self,
        sig_ref: SigRef,
        func_addr: Value,
        callee_vmctx: Value,
        call_args: &[Value],
    ) -> Inst {
        let mut real_call_args = Vec::with_capacity(call_args.len() + 2);
        let caller_vmctx = self
            .builder
            .func
            .special_param(ArgumentPurpose::VMContext)
            .unwrap();

        // First append the callee and caller vmctx addresses.
        real_call_args.push(callee_vmctx);
        real_call_args.push(caller_vmctx);

        // Then append the regular call arguments.
        real_call_args.extend_from_slice(call_args);

        self.indirect_call_inst(sig_ref, func_addr, &real_call_args)
    }

    fn direct_call_inst(&mut self, callee: FuncRef, args: &[Value]) -> Inst {
        if self.tail {
            self.builder.ins().return_call(callee, args)
        } else {
            let inst = self.builder.ins().call(callee, args);
            let results: SmallVec<[_; 4]> = self
                .builder
                .func
                .dfg
                .inst_results(inst)
                .iter()
                .copied()
                .collect();
            for (i, val) in results.into_iter().enumerate() {
                if self
                    .env
                    .func_ref_result_needs_stack_map(self.builder.func, callee, i)
                {
                    self.builder.declare_value_needs_stack_map(val);
                }
            }
            inst
        }
    }

    fn indirect_call_inst(&mut self, sig_ref: SigRef, func_addr: Value, args: &[Value]) -> Inst {
        if self.tail {
            self.builder
                .ins()
                .return_call_indirect(sig_ref, func_addr, args)
        } else {
            let inst = self.builder.ins().call_indirect(sig_ref, func_addr, args);
            let results: SmallVec<[_; 4]> = self
                .builder
                .func
                .dfg
                .inst_results(inst)
                .iter()
                .copied()
                .collect();
            for (i, val) in results.into_iter().enumerate() {
                if self.env.sig_ref_result_needs_stack_map(sig_ref, i) {
                    self.builder.declare_value_needs_stack_map(val);
                }
            }
            inst
        }
    }
}
