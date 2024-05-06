mod impls;

use super::{VMContext, NS_WASM_BUILTIN};
use cranelift_codegen::ir;
use cranelift_codegen::ir::{AbiParam, ArgumentPurpose, Signature};
use cranelift_codegen::isa::{CallConv, TargetIsa};

macro_rules! foreach_builtin_function {
    ($mac:ident) => {
        $mac! {
            // Returns an index for wasm's `memory.grow` builtin function.
            memory32_grow(vmctx: vmctx, delta: i64, index: i32) -> pointer;
            // Returns an index for wasm's `table.copy` when both tables are locally
            // defined.
            table_copy(vmctx: vmctx, dst_index: i32, src_index: i32, dst: i32, src: i32, len: i32);
            // Returns an index for wasm's `table.init`.
            table_init(vmctx: vmctx, table: i32, elem: i32, dst: i32, src: i32, len: i32);
            // Returns an index for wasm's `elem.drop`.
            elem_drop(vmctx: vmctx, elem: i32);
            // Returns an index for wasm's `memory.copy`
            memory_copy(vmctx: vmctx, dst_index: i32, dst: i64, src_index: i32, src: i64, len: i64);
            // Returns an index for wasm's `memory.fill` instruction.
            memory_fill(vmctx: vmctx, memory: i32, dst: i64, val: i32, len: i64);
            // Returns an index for wasm's `memory.init` instruction.
            memory_init(vmctx: vmctx, memory: i32, data: i32, dst: i64, src: i32, len: i32);
            // Returns a value for wasm's `ref.func` instruction.
            ref_func(vmctx: vmctx, func: i32) -> pointer;
            // Returns an index for wasm's `data.drop` instruction.
            data_drop(vmctx: vmctx, data: i32);
            // Returns a table entry after lazily initializing it.
            table_get_lazy_init_func_ref(vmctx: vmctx, table: i32, index: i32) -> pointer;
            // Returns an index for Wasm's `table.grow` instruction for `funcref`s.
            table_grow_func_ref(vmctx: vmctx, table: i32, delta: i32, init: pointer) -> i32;
            // Returns an index for Wasm's `table.fill` instruction for `funcref`s.
            table_fill_func_ref(vmctx: vmctx, table: i32, dst: i32, val: pointer, len: i32);
            // Returns an index for wasm's `memory.atomic.notify` instruction.
            memory_atomic_notify(vmctx: vmctx, memory: i32, addr: i64, count: i32) -> i32;
            // Returns an index for wasm's `memory.atomic.wait32` instruction.
            memory_atomic_wait32(vmctx: vmctx, memory: i32, addr: i64, expected: i32, timeout: i64) -> i32;
            // Returns an index for wasm's `memory.atomic.wait64` instruction.
            memory_atomic_wait64(vmctx: vmctx, memory: i32, addr: i64, expected: i64, timeout: i64) -> i32;
            // // Invoked before malloc returns.
            // check_malloc(vmctx: vmctx, addr: i32, len: i32) -> i32;
            // // Invoked before the free returns.
            // check_free(vmctx: vmctx, addr: i32) -> i32;
            // // Invoked before a load is executed.
            // check_load(vmctx: vmctx, num_bytes: i32, addr: i32, offset: i32) -> i32;
            // // Invoked before a store is executed.
            // check_store(vmctx: vmctx, num_bytes: i32, addr: i32, offset: i32) -> i32;
            // // Invoked after malloc is called.
            // malloc_start(vmctx: vmctx);
            // // Invoked after free is called.
            // free_start(vmctx: vmctx);
            // // Invoked when wasm stack pointer is updated.
            // update_stack_pointer(vmctx: vmctx, value: i32);
            // // Invoked before memory.grow is called.
            // update_mem_size(vmctx: vmctx, num_bytes: i32);
            // // Drop a non-stack GC reference (eg an overwritten table entry)
            // // once it will no longer be used again. (Note: `val` is not a
            // // `reference` because it needn't appear in any stack maps, as it
            // // must not be live after this call.)
            // drop_gc_ref(vmctx: vmctx, val: pointer);
            // // Do a GC, treating the optional `root` as a GC root and returning
            // // the updated `root` (so that, in the case of moving collectors,
            // // callers have a valid version of `root` again).
            // gc(vmctx: vmctx, root: reference) -> reference;
            // // Implementation of Wasm's `global.get` instruction for globals
            // // containing GC references.
            // gc_ref_global_get(vmctx: vmctx, global: i32) -> reference;
            // // Implementation of Wasm's `global.set` instruction for globals
            // // containing GC references.
            // gc_ref_global_set(vmctx: vmctx, global: i32, val: reference);
            // // Returns an index for Wasm's `table.grow` instruction for GC references.
            // table_grow_gc_ref(vmctx: vmctx, table: i32, delta: i32, init: reference) -> i32;
            // // Returns an index for Wasm's `table.fill` instruction for GC references.
            // table_fill_gc_ref(vmctx: vmctx, table: i32, dst: i32, val: reference, len: i32);
        }
    };
}

macro_rules! declare_function_signatures {
    ($(
        $( #[$attr:meta] )*
        $name:ident( $( $pname:ident: $param:ident ),* ) $( -> $result:ident )?;
    )*) => {
        $(impl BuiltinFunctions {
            $( #[$attr] )*
            pub(crate) fn $name(&mut self, func: &mut ir::Function) -> ir::FuncRef {
                self.load_builtin(func, BuiltinFunctionIndex::$name())
            }
        })*
    };
}

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
        #[allow(missing_docs)]
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

foreach_builtin_function!(declare_function_signatures);
foreach_builtin_function!(declare_indexes);

/// Helper structure for creating a `Signature` for all builtins.
pub struct BuiltinFunctionSignatures {
    pointer_type: ir::Type,
    reference_type: ir::Type,
    call_conv: CallConv,
}

impl BuiltinFunctionSignatures {
    pub fn new(isa: &dyn TargetIsa) -> Self {
        Self {
            pointer_type: isa.pointer_type(),
            reference_type: match isa.pointer_type() {
                ir::types::I32 => ir::types::R32,
                ir::types::I64 => ir::types::R64,
                _ => panic!(),
            },
            call_conv: CallConv::triple_default(isa.triple()),
        }
    }

    /// Returns the AbiParam for builtin functions `vmctx` arguments.
    /// This function is used in the `signatures` macro below.
    fn vmctx(&self) -> AbiParam {
        AbiParam::special(self.pointer_type, ArgumentPurpose::VMContext)
    }

    /// Returns the AbiParam for builtin functions `reference` arguments/returns.
    /// This function is used in the `signatures` macro below.
    fn reference(&self) -> AbiParam {
        AbiParam::new(self.reference_type)
    }

    /// Returns the AbiParam for builtin functions `pointer` arguments/returns.
    /// This function is used in the `signatures` macro below.
    fn pointer(&self) -> AbiParam {
        AbiParam::new(self.pointer_type)
    }

    /// Returns the AbiParam for builtin functions `i32` arguments/returns.
    /// This function is used in the `signatures` macro below.
    fn i32(&self) -> AbiParam {
        // Some platform ABIs require i32 values to be zero- or sign-
        // extended to the full register width.  We need to indicate
        // this here by using the appropriate .uext or .sext attribute.
        // The attribute can be added unconditionally; platforms whose
        // ABI does not require such extensions will simply ignore it.
        // Note that currently all i32 arguments or return values used
        // by builtin functions are unsigned, so we always use .uext.
        // If that ever changes, we will have to add a second type
        // marker here.
        AbiParam::new(ir::types::I32).uext()
    }

    /// Returns the AbiParam for builtin functions `i64` arguments/returns.
    /// This function is used in the `signatures` macro below.
    fn i64(&self) -> AbiParam {
        AbiParam::new(ir::types::I64)
    }

    pub fn signature(&self, builtin: BuiltinFunctionIndex) -> Signature {
        let mut _cur = 0;
        macro_rules! iter {
            (
                $(
                    $( #[$attr:meta] )*
                    $name:ident( $( $pname:ident: $param:ident ),* ) $( -> $result:ident )?;
                )*
            ) => {
                $(
                    $( #[$attr] )*
                    if _cur == builtin.as_u32() {
                        return Signature {
                            params: ::alloc::vec![ $( self.$param() ),* ],
                            returns: ::alloc::vec![ $( self.$result() )? ],
                            call_conv: self.call_conv,
                        };
                    }
                    _cur += 1;
                )*
            };
        }

        foreach_builtin_function!(iter);

        unreachable!();
    }
}

/// An index type for builtin functions.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BuiltinFunctionIndex(u32);

impl BuiltinFunctionIndex {
    /// Create a new `BuiltinFunctionIndex` from its index
    pub const fn from_u32(i: u32) -> Self {
        Self(i)
    }

    /// Return the index as an u32 number.
    pub const fn as_u32(&self) -> u32 {
        self.0
    }
}

pub struct BuiltinFunctions {
    types: BuiltinFunctionSignatures,

    builtins:
        [Option<ir::FuncRef>; BuiltinFunctionIndex::builtin_functions_total_number() as usize],
}

impl BuiltinFunctions {
    pub fn new(isa: &dyn TargetIsa) -> Self {
        Self {
            types: BuiltinFunctionSignatures::new(isa),
            builtins: [None; BuiltinFunctionIndex::builtin_functions_total_number() as usize],
        }
    }

    fn load_builtin(
        &mut self,
        func: &mut ir::Function,
        index: BuiltinFunctionIndex,
    ) -> ir::FuncRef {
        let cache = &mut self.builtins[index.as_u32() as usize];
        if let Some(f) = cache {
            return *f;
        }
        let signature = func.import_signature(self.types.signature(index));
        let name =
            ir::ExternalName::User(func.declare_imported_user_function(ir::UserExternalName {
                namespace: NS_WASM_BUILTIN,
                index: index.as_u32(),
            }));
        let f = func.import_function(ir::ExtFuncData {
            name,
            signature,
            colocated: true,
        });
        *cache = Some(f);
        f
    }
}

macro_rules! define_builtin_array {
    (
        $(
            $( #[$attr:meta] )*
            $name:ident( $( $pname:ident: $param:ident ),* ) $( -> $result:ident )?;
        )*
    ) => {
        /// An array that stores addresses of builtin functions. We translate code
        /// to use indirect calls. This way, we don't have to patch the code.
        #[repr(C)]
        pub struct VMBuiltinFunctionsArray {
            $(
                $name: unsafe extern "C" fn(
                    $(define_builtin_array!(@ty $param)),*
                ) $( -> define_builtin_array!(@ty $result))?,
            )*
        }

        impl VMBuiltinFunctionsArray {
            #[allow(unused_doc_comments)]
            pub const INIT: VMBuiltinFunctionsArray = VMBuiltinFunctionsArray {
                $(
                    $name: crate::rt::builtins::impls::$name,
                )*
            };
        }
    };

    (@ty i32) => (u32);
    (@ty i64) => (u64);
    (@ty reference) => (*mut u8);
    (@ty pointer) => (*mut u8);
    (@ty vmctx) => (*mut VMContext);
}

foreach_builtin_function!(define_builtin_array);
