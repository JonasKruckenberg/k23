use crate::translate_inst::Reachability;
use crate::FuncTranslationEnvironment;
use cranelift_codegen::cursor::{Cursor, FuncCursor};
use cranelift_codegen::ir;
use cranelift_codegen::ir::{
    Endianness, Fact, InstBuilder, MemFlags, MemoryType, RelSourceLoc, TrapCode, Type, Value,
};
use cranelift_entity::entity_impl;
use cranelift_frontend::FunctionBuilder;

/// An opaque reference to a [`HeapData`][crate::HeapData].
///
/// While the order is stable, it is arbitrary.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Heap(u32);
entity_impl!(Heap, "heap");

/// WebAssembly operates on a small set of bounded memory areas.
/// These are represented as `Heap`s following the design of Wasmtime.
///
///
/// ## Heaps and virtual memory
///
/// In traditional WASM engines, heaps are allocated through the host OS and therefore need additional
/// protections such as guard pages. In our case however, where WASM instances *are* userspace programs
/// we can use virtual memory for memory isolation.
///
/// In the case that there is only one linear memory configured, the heap starts at a
/// dynamically determined start address (after the VMcontext and data) and runs until the kernel memory space starts.
/// In case that are multiple linear memories the address space is equally split among the heaps
/// (excluding heaps that have a configured max size).
///
/// Heaps are made up of two address ranges: *mapped pages* and *unmapped pages*:
/// When first initializing **min_size** pages are automatically mapped to the processes' address space,
/// followed by unmapped pages until the heap ends.
///
/// A heap starts out with all the address space it will ever need, so
/// it never moves to a different address. At the base address is a number of
/// mapped pages corresponding to the heap's current size. Then follows a number
/// of unmapped pages where the heap can grow up to its maximum size.
///
/// Heaps therefore correspond 1:1 to the processes' memory.
pub struct HeapData {
    /// The address of the start of the heap's storage.
    pub base: ir::GlobalValue,

    /// Guaranteed minimum heap size in **pages**. Heap accesses before `min_size`
    /// don't need bounds checking.
    pub min_size: u64,

    /// The maximum heap size in **pages**.
    ///
    /// Heap accesses larger than this will always trap.
    pub max_size: u64,

    /// Whether this is a 64-bit memory
    pub memory64: bool,

    /// The index type for the heap.
    pub index_type: ir::Type,

    /// The memory type for the pointed-to memory, if using proof-carrying code.
    pub memory_type: Option<ir::MemoryType>,
}

impl HeapData {
    pub fn get_addr_for_index(
        &self,
        pos: &mut FunctionBuilder,
        index: Value,
        access_size: u8,
        memarg: wasmparser::MemArg,
        env: &dyn FuncTranslationEnvironment,
    ) -> crate::Result<Reachability> {
        let pcc = env.proof_carrying_code();

        let index_ty = pos.func.dfg.value_type(index);
        let addr_ty = env.target_config().pointer_type();
        let index = cast_index_to_pointer_ty(pos.cursor(), index, index_ty, addr_ty, pcc);

        // optimization for when we can statically assert this access will trap
        if memarg.offset + u64::from(access_size) > self.max_size {
            pos.ins().trap(TrapCode::HeapOutOfBounds);
            return Ok(Reachability::Unreachable);
        }

        // Load the heap base address
        let base = pos.ins().global_value(addr_ty, self.base);
        let base_and_index = pos.ins().iadd(base, index);

        let addr = if memarg.offset == 0 {
            base_and_index
        } else {
            let offset = pos.ins().iconst(addr_ty, i64::try_from(memarg.offset)?);
            let base_index_and_offset = pos.ins().iconst(addr_ty, i64::try_from(memarg.offset)?);

            if let Some(ty) = self.memory_type {
                self.emit_memarg_offset_proofs(
                    pos,
                    ty,
                    offset,
                    base_index_and_offset,
                    addr_ty,
                    memarg,
                );
            }

            base_index_and_offset
        };

        let mut flags = MemFlags::new()
            .with_endianness(Endianness::Little)
            .with_alias_region(Some(ir::AliasRegion::Heap));

        if let Some(ty) = self.memory_type {
            self.emit_base_proofs(pos, ty, base, base_and_index);

            // Proof-carrying code is enabled; check this memory access.
            flags.set_checked();
        }

        Ok(Reachability::Reachable(addr, flags))
    }

    fn emit_base_proofs(
        &self,
        pos: &mut FunctionBuilder,
        memory_ty: MemoryType,
        base: Value,
        base_and_index: Value,
    ) {
        // emit pcc fact for heap base
        pos.func.dfg.facts[base] = Some(Fact::Mem {
            ty: memory_ty,
            min_offset: 0,
            max_offset: 0,
            nullable: false,
        });

        // emit pcc fact for base + index
        pos.func.dfg.facts[base_and_index] = Some(Fact::Mem {
            ty: memory_ty,
            min_offset: 0,
            max_offset: u64::from(u32::MAX),
            nullable: false,
        });
    }

    fn emit_memarg_offset_proofs(
        &self,
        pos: &mut FunctionBuilder,
        memory_ty: MemoryType,
        offset: Value,
        base_index_and_offset: Value,
        addr_ty: Type,
        memarg: wasmparser::MemArg,
    ) {
        // emit pcc fact for offset
        pos.func.dfg.facts[offset] = Some(Fact::constant(
            u16::try_from(addr_ty.bits()).unwrap(),
            u64::from(memarg.offset),
        ));

        // emit pcc fact for base + index + offset
        pos.func.dfg.facts[base_index_and_offset] = Some(Fact::Mem {
            ty: memory_ty,
            min_offset: u64::from(memarg.offset),
            max_offset: u64::from(u32::MAX).checked_add(memarg.offset).unwrap(),
            nullable: false,
        });
    }
}

fn cast_index_to_pointer_ty(
    mut pos: FuncCursor,
    index: Value,
    index_ty: Type,
    pointer_ty: Type,
    pcc: bool,
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

    // Add a range fact on the extended value.
    if pcc {
        pos.func.dfg.facts[extended_index] = Some(Fact::max_range_for_width_extended(
            u16::try_from(index_ty.bits()).unwrap(),
            u16::try_from(pointer_ty.bits()).unwrap(),
        ));
    }

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
