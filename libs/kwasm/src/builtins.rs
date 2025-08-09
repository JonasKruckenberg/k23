// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use cranelift_entity::entity_impl;

/// Iterate over all builtin functions and call the provided macro for each.
macro_rules! foreach_builtin_function {
    ($mac:ident) => {
        $mac! {
            // Wasm's `memory.grow` instruction
            memory_grow(vmctx: vmctx, memory_index: u32, delta: u64) -> pointer;
            // Wasm's `memory.init` instruction
            memory_init(vmctx: vmctx, memory_index: u32, data_index: u32, dst: u64, src: u32, len: u32);
            // Wasm's `memory.copy` instruction
            memory_copy(vmctx: vmctx, dst_index: u32, dst: u64, src_index: u32, src: u64, len: u64);
            // Wasm's `memory.fill` instruction
            memory_fill(vmctx: vmctx, memory_index: u32, dst: u64, val: u32, len: u64);
            // Wasm's `data.drop` instruction
            data_drop(vmctx: vmctx, data_index: u32);

            // Wasm's `table.grow` instruction for `funcref`s.
            // table_grow_func_ref(vmctx: vmctx, table: u32, delta: u64, init: pointer) -> pointer;
            // Wasm's `table.init` instruction
            table_init(vmctx: vmctx, table_index: u32, elem_index: u32, dst: u64, src: u64, len: u64);
            // Wasm's `table.copy` instruction
            table_copy(vmctx: vmctx, dst_index: u32, src_index: u32, dst: u64, src: u64, len: u64);
            // Returns an index for Wasm's `table.fill` instruction for `funcref`s.
            table_fill_func_ref(vmctx: vmctx, table_index: u32, dst: u64, val: pointer, len: u64);
            // Wasm's `elem.drop` instruction
            elem_drop(vmctx: vmctx, elem_index: u32);

            // Wasm's `memory.atomic.notify` instruction.
            memory_atomic_notify(vmctx: vmctx, memory_index: u32, addr: u64, count: u32) -> u64;
            // Wasm's `memory.atomic.wait32` instruction.
            memory_atomic_wait32(vmctx: vmctx, memory_index: u32, addr: u64, expected: u32, timeout: u64) -> u64;
            // Wasm's `memory.atomic.wait64` instruction.
            memory_atomic_wait64(vmctx: vmctx, memory_index: u32, addr: u64, expected: u64, timeout: u64) -> u64;

            // Raises an unconditional trap where the trap information must have
            // been previously filled in.
            raise(vmctx: vmctx);
        }
    };
}
pub(crate) use foreach_builtin_function;

/// An index type for builtin functions.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BuiltinFunctionIndex(u32);
entity_impl!(BuiltinFunctionIndex);

macro_rules! declare_indexes {
    (
        $(
            $( #[$attr:meta] )*
            $name:ident( $( $pname:ident: $param:ident ),* ) $( -> $result:ident )?;
        )*
    ) => {
        impl BuiltinFunctionIndex {
            declare_indexes!(
                @indices;
                0;
                $( $( #[$attr] )* $name; )*
            );

            /// Returns a symbol name for this builtin.
            pub fn name(&self) -> &'static str {
                $(
                    $( #[$attr] )*
                    if *self == BuiltinFunctionIndex::$name() {
                        return stringify!($name);
                    }
                )*
                unreachable!()
            }
        }
    };

    // Base case: no more indices to declare, so define the total number of
    // function indices.
    (
        @indices;
        $len:expr;
    ) => {
        /// Returns the total number of builtin functions.
        pub const fn builtin_functions_total_number() -> u32 {
            $len
        }
    };

    // Recursive case: declare the next index, and then keep declaring the rest of
    // the indices.
    (
         @indices;
         $index:expr;
         $( #[$this_attr:meta] )*
         $this_name:ident;
         $(
             $( #[$rest_attr:meta] )*
             $rest_name:ident;
         )*
    ) => {
        $( #[$this_attr] )*
        pub const fn $this_name() -> Self {
            Self($index)
        }

        declare_indexes!(
            @indices;
            ($index + 1);
            $( $( #[$rest_attr] )* $rest_name; )*
        );
    }
}

foreach_builtin_function!(declare_indexes);
