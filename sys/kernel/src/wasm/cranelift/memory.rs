// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use cranelift_codegen::cursor::{Cursor, FuncCursor};
use cranelift_codegen::ir;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{InstBuilder, MemFlagsData, RelSourceLoc, TrapCode, Type, Value};
use cranelift_frontend::FunctionBuilder;
use wasmparser::MemArg;

use crate::wasm::cranelift::code_translator::Reachability;
use crate::wasm::cranelift::env::TranslationEnvironment;
use crate::wasm::cranelift::utils::{heap_alias_region, index_type_to_ir_type};
use crate::wasm::translate::IndexType;
use crate::wasm::trap::TRAP_HEAP_MISALIGNED;

#[derive(Debug, Clone)]
pub struct CraneliftMemory {
    /// The address of the start of the heap's storage.
    pub base_gv: ir::GlobalValue,
    /// The index type for the heap.
    pub index_type: IndexType,
    /// Heap bound in bytes. The offset-guard pages are allocated after the
    /// bound.
    pub bound: u64,
    /// Guaranteed minimum heap size in bytes. Heap accesses before `min_size`
    /// don't need bounds checking.
    pub min_size: u64,
    /// The maximum heap size in bytes.
    ///
    /// Heap accesses larger than this will always trap.
    pub max_size: Option<u64>,
    /// Size in bytes of the offset-guard pages following the heap.
    pub offset_guard_size: u64,
    /// The log2 of this memory's page size.
    pub page_size_log2: u8,
}

impl CraneliftMemory {
    /// Returns `None` when the Wasm access will unconditionally trap.
    ///
    /// Returns `(flags, wasm_addr, native_addr)`.
    pub fn prepare_addr(
        &self,
        builder: &mut FunctionBuilder,
        index: Value,
        access_size: u8,
        memarg: &MemArg,
        env: &mut TranslationEnvironment,
    ) -> Reachability<(MemFlagsData, Value, Value)> {
        let addr = if let Ok(offset) = u32::try_from(memarg.offset) {
            // If our offset fits within a u32, then we can place it into the
            // offset immediate of the `heap_addr` instruction.
            self.bounds_check_and_compute_addr(builder, index, offset, access_size, env)
        } else {
            // If the offset doesn't fit within a u32, then we can't pass it
            // directly into `heap_addr`.
            let offset = builder.ins().iconst(
                index_type_to_ir_type(self.index_type),
                i64::try_from(memarg.offset).unwrap(),
            );
            let adjusted_index =
                builder
                    .ins()
                    .uadd_overflow_trap(index, offset, TrapCode::HEAP_OUT_OF_BOUNDS);
            self.bounds_check_and_compute_addr(builder, adjusted_index, 0, access_size, env)
        };

        match addr {
            Reachability::Unreachable => Reachability::Unreachable,
            Reachability::Reachable(addr) => {
                // Note that we don't set `is_aligned` here, even if the load instruction's
                // alignment immediate may say it's aligned, because WebAssembly's
                // immediate field is just a hint, while Cranelift's aligned flag needs a
                // guarantee. WebAssembly memory accesses are always little-endian.
                let mut flags = MemFlagsData::new();
                flags.set_endianness(ir::Endianness::Little);

                // The access occurs to the `heap` disjoint category of abstract
                // state. This may allow alias analysis to merge redundant loads,
                // etc. when heap accesses occur interleaved with other (table,
                // vmctx, stack) accesses.
                flags.set_alias_region(Some(heap_alias_region(builder.func)));

                Reachability::Reachable((flags, index, addr))
            }
        }
    }

    /// Like `prepare_addr` but for atomic accesses.
    ///
    /// Returns `None` when the Wasm access will unconditionally trap.
    pub fn prepare_atomic_addr(
        &self,
        builder: &mut FunctionBuilder,
        index: Value,
        loaded_bytes: u8,
        memarg: &MemArg,
        env: &mut TranslationEnvironment,
    ) -> Reachability<(MemFlagsData, Value, Value)> {
        // Atomic addresses must all be aligned correctly, and for now we check
        // alignment before we check out-of-bounds-ness. The order of this check may
        // need to be updated depending on the outcome of the official threads
        // proposal itself.
        //
        // Note that with an offset>0 we generate an `iadd_imm` where the result is
        // thrown away after the offset check. This may truncate the offset and the
        // result may overflow as well, but those conditions won't affect the
        // alignment check itself. This can probably be optimized better and we
        // should do so in the future as well.
        if loaded_bytes > 1 {
            let effective_addr = if memarg.offset == 0 {
                index
            } else {
                builder
                    .ins()
                    .iadd_imm(index, i64::try_from(memarg.offset).unwrap())
            };
            debug_assert!(loaded_bytes.is_power_of_two());
            let misalignment = builder.ins().band_imm(
                effective_addr,
                i64::from(loaded_bytes.checked_sub(1).unwrap()),
            );
            let f = builder.ins().icmp_imm(IntCC::NotEqual, misalignment, 0);
            builder.ins().trapnz(f, TRAP_HEAP_MISALIGNED);
        }

        self.prepare_addr(builder, index, loaded_bytes, memarg, env)
    }

    fn bounds_check_and_compute_addr(
        &self,
        builder: &mut FunctionBuilder,
        // Dynamic operand indexing into the memory.
        index: Value,
        // Static immediate added to the index.
        offset: u32,
        // Static size of the heap access.
        access_size: u8,
        env: &mut TranslationEnvironment,
    ) -> Reachability<Value> {
        let index = cast_index_to_pointer_ty(
            index,
            index_type_to_ir_type(self.index_type),
            env.pointer_type(),
            &mut builder.cursor(),
        );

        let spectre_mitigations_enabled = env.heap_access_spectre_mitigation();
        // Cannot overflow because we are widening to `u64`.
        // TODO when memory64 is supported this needs to be handles correctly
        let offset_and_size = u64::from(offset) + u64::from(access_size);

        let host_page_size_log2 = env.target_isa().page_size_align_log2();
        let can_use_virtual_memory = self.page_size_log2 >= host_page_size_log2;
        assert!(
            can_use_virtual_memory,
            "k23's memories require the ability to use virtual memory"
        );

        if offset_and_size > self.bound {
            // 1. First special case: trap immediately if `offset + access_size >
            //    bound`, since we will end up being out-of-bounds regardless of the
            //    given `index`.
            builder.ins().trap(TrapCode::HEAP_OUT_OF_BOUNDS);
            Reachability::Unreachable
        } else if index_type_to_ir_type(self.index_type) == ir::types::I32
            && u64::from(u32::MAX)
                <= self
                    .bound
                    .saturating_add(self.offset_guard_size)
                    .saturating_add(offset_and_size)
        {
            // 2. Second special case for when we can completely omit explicit
            //    bounds checks for 32-bit static memories.
            //
            //    First, let's rewrite our comparison to move all the constants
            //    to one side:
            //
            //            index + offset + access_size > bound
            //        ==> index > bound - (offset + access_size)
            //
            //    We know the subtraction on the right-hand side won't wrap because
            //    we didn't hit the first special case.
            //
            //    Additionally, we add our guard pages (if any) to the right-hand
            //    side, since we can rely on the virtual memory subsystem at runtime
            //    to catch out-of-bound accesses within the range `bound .. bound +
            //    guard_size`. So now we are dealing with
            //
            //        index > bound + guard_size - (offset + access_size)
            //
            //    Note that `bound + guard_size` cannot overflow for
            //    correctly-configured heaps, as otherwise the heap wouldn't fit in
            //    a 64-bit memory space.
            //
            //    The complement of our should-this-trap comparison expression is
            //    the should-this-not-trap comparison expression:
            //
            //        index <= bound + guard_size - (offset + access_size)
            //
            //    If we know the right-hand side is greater than or equal to
            //    `u32::MAX`, then
            //
            //        index <= u32::MAX <= bound + guard_size - (offset + access_size)
            //
            //    This expression is always true when the heap is indexed with
            //    32-bit integers because `index` cannot be larger than
            //    `u32::MAX`. This means that `index` is always either in bounds or
            //    within the guard page region, neither of which require emitting an
            //    explicit bounds check.

            Reachability::Reachable(self.compute_addr(
                &mut builder.cursor(),
                env.pointer_type(),
                index,
                offset,
            ))
        } else {
            // 3. General case for static memories.
            //
            //    We have to explicitly test whether
            //
            //        index > bound - (offset + access_size)
            //
            //    and trap if so.
            //
            //    Since we have to emit explicit bounds checks, we might as well be
            //    precise, not rely on the virtual memory subsystem at all, and not
            //    factor in the guard pages here.
            // NB: this subtraction cannot wrap because we didn't hit the first
            // special case.
            let adjusted_bound = self.bound.checked_sub(offset_and_size).unwrap();
            let adjusted_bound_value = builder
                .ins()
                .iconst(env.pointer_type(), i64::try_from(adjusted_bound).unwrap());
            let oob = builder
                .ins()
                .icmp(IntCC::UnsignedGreaterThan, index, adjusted_bound_value);
            Reachability::Reachable(self.explicit_check_oob_condition_and_compute_addr(
                builder,
                env.pointer_type(),
                index,
                offset,
                spectre_mitigations_enabled,
                oob,
            ))
        }
    }

    fn explicit_check_oob_condition_and_compute_addr(
        &self,
        builder: &mut FunctionBuilder,
        addr_ty: Type,
        index: Value,
        offset: u32,
        // Whether Spectre mitigations are enabled for heap accesses.
        spectre_mitigations_enabled: bool,
        // The `i8` boolean value that is non-zero when the heap access is out of
        // bounds (and therefore we should trap) and is zero when the heap access is
        // in bounds (and therefore we can proceed).
        oob_condition: Value,
    ) -> Value {
        if !spectre_mitigations_enabled {
            builder
                .ins()
                .trapnz(oob_condition, TrapCode::HEAP_OUT_OF_BOUNDS);
        }
        let mut addr = self.compute_addr(&mut builder.cursor(), addr_ty, index, offset);

        if spectre_mitigations_enabled {
            let null = builder.ins().iconst(addr_ty, 0);
            addr = builder
                .ins()
                .select_spectre_guard(oob_condition, null, addr);
        }

        addr
    }

    fn compute_addr(
        &self,
        pos: &mut FuncCursor,
        addr_ty: Type,
        index: Value,
        offset: u32,
    ) -> Value {
        debug_assert_eq!(pos.func.dfg.value_type(index), addr_ty);

        let heap_base = pos.ins().global_value(addr_ty, self.base_gv);
        let base_and_index = pos.ins().iadd(heap_base, index);

        if offset == 0 {
            base_and_index
        } else {
            // NB: The addition of the offset immediate must happen *before* the
            // `select_spectre_guard`, if any. If it happens after, then we
            // potentially are letting speculative execution read the whole first
            // 4GiB of memory.
            let offset_val = pos.ins().iconst(addr_ty, i64::from(offset));
            pos.ins().iadd(base_and_index, offset_val)
        }
    }
}

fn cast_index_to_pointer_ty(
    index: Value,
    index_ty: Type,
    pointer_ty: Type,
    pos: &mut FuncCursor,
) -> Value {
    if index_ty == pointer_ty {
        return index;
    }
    // Note that using 64-bit heaps on a 32-bit host is not currently supported,
    // would require at least a bounds check here to ensure that the truncation
    // from 64-to-32 bits doesn't lose any upper bits. For now though we're
    // mostly interested in the 32-bit-heaps-on-64-bit-hosts cast.
    assert!(index_ty.bits() < pointer_ty.bits());

    // Convert `index` to `addr_ty`.
    let extended_index = pos.ins().uextend(pointer_ty, index);

    // Add debug value-label alias so that debuginfo can name the extended
    // value as the address
    let loc = pos.srcloc();
    let loc = RelSourceLoc::from_base_offset(pos.func.params.base_srcloc(), loc);
    pos.func
        .stencil
        .dfg
        .add_value_label_alias(extended_index, loc, index);

    extended_index
}
