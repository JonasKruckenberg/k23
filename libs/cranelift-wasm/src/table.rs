use cranelift_codegen::cursor::FuncCursor;
use cranelift_codegen::ir;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::immediates::Imm64;
use cranelift_codegen::ir::{Fact, InstBuilder, MemoryType, Value};
use cranelift_entity::entity_impl;
use cranelift_frontend::FunctionBuilder;

/// Size of a WebAssembly table, in elements.
#[derive(Clone)]
pub enum TableSize {
    /// Non-resizable table.
    Static {
        /// Non-resizable tables have a constant size known at compile time.
        bound: u32,
    },
    /// Resizable table.
    Dynamic {
        /// Resizable tables declare a Cranelift global value to load the
        /// current size from.
        bound_gv: ir::GlobalValue,
    },
}

impl TableSize {
    /// Get a CLIF value representing the current bounds of this table.
    pub fn bound(&self, mut pos: FuncCursor, index_ty: ir::Type) -> ir::Value {
        match *self {
            TableSize::Static { bound } => pos.ins().iconst(index_ty, Imm64::new(i64::from(bound))),
            TableSize::Dynamic { bound_gv } => pos.ins().global_value(index_ty, bound_gv),
        }
    }
}

/// An opaque reference to a [`TableData`][crate::TableData].
///
/// While the order is stable, it is arbitrary.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Table(u32);
entity_impl!(Table, "table");

pub struct TableData {
    /// The start address of the table.
    pub base: ir::GlobalValue,

    /// The size of the table, in elements.
    pub bound: TableSize,

    /// The size of a table element, in bytes.
    pub element_size: u32,

    /// The memory type for the pointed-to memory, if using proof-carrying code.
    pub memory_type: Option<MemoryType>,
}

impl TableData {
    pub fn get_addr_for_element(
        &self,
        pos: &mut FunctionBuilder,
        mut index: ir::Value,
        addr_ty: ir::Type,
    ) -> (ir::Value, ir::MemFlags) {
        let index_ty = pos.func.dfg.value_type(index);

        // Start with the bounds check. Trap if `index + 1 > bound`.
        let bound = self.bound.bound(pos.cursor(), index_ty);

        // `index > bound - 1` is the same as `index >= bound`.
        let oob = pos
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, index, bound);

        // Convert `index` to `addr_ty`.
        if index_ty != addr_ty {
            index = pos.ins().uextend(addr_ty, index);
        }

        // Load the table base address
        let base = pos.ins().global_value(addr_ty, self.base);

        let element_size = self.element_size;
        let offset = if element_size == 1 {
            index
        } else if element_size.is_power_of_two() {
            pos.ins()
                .ishl_imm(index, i64::from(element_size.trailing_zeros()))
        } else {
            pos.ins().imul_imm(index, element_size as i64)
        };

        let element_addr = pos.ins().iadd(base, offset);

        let base_flags = ir::MemFlags::new()
            .with_aligned()
            .with_alias_region(Some(ir::AliasRegion::Table));

        if let Some(ty) = self.memory_type {
            self.emit_proofs(pos, ty, base, element_addr);
        }

        // Short-circuit the computed table element address to a null pointer
        // when out-of-bounds. The consumer of this address will trap when
        // trying to access it.
        let zero = pos.ins().iconst(addr_ty, 0);

        (
            pos.ins().select_spectre_guard(oob, zero, element_addr),
            base_flags.with_trap_code(Some(ir::TrapCode::TableOutOfBounds)),
        )
    }

    fn emit_proofs(
        &self,
        pos: &mut FunctionBuilder,
        memory_ty: MemoryType,
        base: Value,
        element_addr: Value,
    ) {
        // emit pcc fact for table base
        pos.func.dfg.facts[base] = Some(Fact::Mem {
            ty: memory_ty,
            min_offset: 0,
            max_offset: 0,
            nullable: false,
        });

        // emit pcc fact for element_addr
        pos.func.dfg.facts[element_addr] = Some(Fact::Mem {
            ty: memory_ty,
            min_offset: 0,
            max_offset: u64::from(u32::MAX),
            nullable: false,
        });
    }
}
