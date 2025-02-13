mod builtins;
mod code_translator;
mod compiler;
mod env;
mod func_translator;
mod memory;
mod state;
mod utils;

use crate::wasm::trap::TRAP_TABLE_OUT_OF_BOUNDS;
pub use compiler::CraneliftCompiler;
use cranelift_codegen::ir;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::immediates::Imm64;
use cranelift_codegen::ir::{InstBuilder, MemFlags};
use cranelift_frontend::FunctionBuilder;

/// The value of a WebAssembly global variable.
#[derive(Clone, Copy)]
pub(crate) enum CraneliftGlobal {
    /// This is a constant global with a value known at compile time.
    Const(ir::Value),
    /// This is a variable in memory that should be referenced through a `GlobalValue`.
    Memory {
        /// The address of the global variable storage.
        gv: ir::GlobalValue,
        /// An offset to add to the address.
        offset: ir::immediates::Offset32,
        /// The global variable's type.
        ty: ir::Type,
    },
    /// This is a global variable that needs to be handled by the environment.
    Custom,
}

#[derive(Debug, Clone)]
pub(crate) struct CraneliftTable {
    /// Global value giving the address of the start of the table.
    pub base_gv: ir::GlobalValue,
    /// The size of the table, in elements.
    pub bound: u64,
    /// The size of a table element, in bytes.
    pub element_size: u32,
}

impl CraneliftTable {
    pub(crate) fn prepare_addr(
        &self,
        builder: &mut FunctionBuilder,
        mut index: ir::Value,
        pointer_type: ir::Type,
        spectre_mitigations_enabled: bool,
    ) -> (ir::Value, MemFlags) {
        let index_ty = builder.func.dfg.value_type(index);

        // Start with the bounds check. Trap if `index + 1 > bound`.
        let bound = builder
            .ins()
            .iconst(index_ty, Imm64::new(i64::try_from(self.bound).unwrap()));
        let oob = builder
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, index, bound);

        if !spectre_mitigations_enabled {
            builder.ins().trapnz(oob, TRAP_TABLE_OUT_OF_BOUNDS);
        }

        // Convert `index` to `addr_ty`.
        if index_ty != pointer_type {
            index = builder.ins().uextend(pointer_type, index);
        }

        // then load the table base address
        let base = builder.ins().global_value(pointer_type, self.base_gv);
        // and calculate `index` * `element_size` to get the element offset
        let offset = if self.element_size == 1 {
            index
        } else if self.element_size.is_power_of_two() {
            // use less expensive shifting instead of slow multiplication
            builder
                .ins()
                .ishl_imm(index, i64::from(self.element_size.trailing_zeros()))
        } else {
            builder.ins().imul_imm(index, i64::from(self.element_size))
        };
        // add both together to get the element address
        let element_addr = builder.ins().iadd(base, offset);

        let base_flags = MemFlags::new()
            .with_aligned()
            .with_alias_region(Some(ir::AliasRegion::Table));

        if spectre_mitigations_enabled {
            // Short-circuit the computed table element address to a null pointer
            // when out-of-bounds. The consumer of this address will trap when
            // trying to access it.
            let zero = builder.ins().iconst(pointer_type, 0);
            (
                builder.ins().select_spectre_guard(oob, zero, element_addr),
                base_flags.with_trap_code(Some(TRAP_TABLE_OUT_OF_BOUNDS)),
            )
        } else {
            (element_addr, base_flags.with_trap_code(None))
        }
    }
}
