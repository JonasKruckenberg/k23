macro_rules! foreach_hostcall_function {
    ($mac:ident) => {
        $mac! {
            /// Returns an index for wasm's `memory.init` instruction.
            memory_init(vmctx: vmctx, memory: i32, data: i32, dst: i64, src: i32, len: i32);
            /// Returns an index for wasm's `memory.grow` builtin function.
            memory_grow(vmctx: vmctx, delta: i64, index: i32) -> pointer;
            /// Returns an index for wasm's `memory.copy`
            memory_copy(vmctx: vmctx, dst_index: i32, dst: i64, src_index: i32, src: i64, len: i64);
            /// Returns an index for wasm's `memory.fill` instruction.
            memory_fill(vmctx: vmctx, memory: i32, dst: i64, val: i32, len: i64);
            /// Returns an index for wasm's `data.drop` instruction.
            data_drop(vmctx: vmctx, data: i32);

            /// Returns an index for wasm's `table.init`.
            table_init(vmctx: vmctx, table: i32, elem: i32, dst: i32, src: i32, len: i32);
            /// Returns an index for wasm's `table.copy` when both tables are locally
            /// defined.
            table_copy(vmctx: vmctx, dst_index: i32, src_index: i32, dst: i32, src: i32, len: i32);

            /// Returns an index for wasm's `memory.atomic.notify` instruction.
            memory_atomic_notify(vmctx: vmctx, memory: i32, addr: i64, count: i32) -> i32;
            /// Returns an index for wasm's `memory.atomic.wait32` instruction.
            memory_atomic_wait32(vmctx: vmctx, memory: i32, addr: i64, expected: i32, timeout: i64) -> i32;
            /// Returns an index for wasm's `memory.atomic.wait64` instruction.
            memory_atomic_wait64(vmctx: vmctx, memory: i32, addr: i64, expected: i64, timeout: i64) -> i32;
        }
    };
}

macro_rules! define_builtin_functions {
    (
        $(
            $( #[$attr:meta] )*
            $name:ident( $( $pname:ident: $param:ident ),* ) $( -> $result:ident )?;
        )*
    ) => {
        struct BuiltinFunctions {
            pointer_type: ir::Type,
            reference_type: ir::Type,
            call_conv: isa::CallConv,
            $(
                $name: Option<ir::SigRef>,
            )*
        }

        impl BuiltinFunctions {
            fn new(
                pointer_type: ir::Type,
                reference_type: ir::Type,
                call_conv: isa::CallConv,
            ) -> Self {
                Self {
                    pointer_type,
                    reference_type,
                    call_conv,
                    $(
                        $name: None,
                    )*
                }
            }

            $(
                fn $name(&mut self, func: &mut Function) -> ir::SigRef {
                    let sig = self.$name.unwrap_or_else(|| {
                        func.import_signature(Signature {
                            params: vec![ $( self.$param() ),* ],
                            returns: vec![ $( self.$result() )? ],
                            call_conv: self.call_conv,
                        })
                    });
                    self.$name = Some(sig);
                    sig
                }
            )*
        }
    };
}
//
// foreach_builtin_function!(define_builtin_functions);
