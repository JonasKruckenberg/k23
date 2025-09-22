// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![expect(unused, reason = "TODO")]

use alloc::vec;

use cranelift_codegen::ir::{self, AbiParam, ArgumentPurpose, Function, Signature, Type, types};
use cranelift_codegen::isa::{CallConv, TargetIsa};
use cranelift_entity::EntityRef;

use crate::builtins::{BuiltinFunctionIndex, foreach_builtin_function};
use crate::compile::NS_BUILTIN;

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

    pub fn load_builtin(
        &mut self,
        func: &mut Function,
        index: BuiltinFunctionIndex,
    ) -> ir::FuncRef {
        let cache = &mut self.builtins[index.index()];
        if let Some(f) = cache {
            return *f;
        }
        let signature = func.import_signature(self.types.host_signature(index));
        let name =
            ir::ExternalName::User(func.declare_imported_user_function(ir::UserExternalName {
                namespace: NS_BUILTIN,
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

macro_rules! declare_function_signatures {
    ($(
        $( #[$attr:meta] )*
        $name:ident( $( $pname:ident: $param:ident ),* ) $( -> $result:ident )?;
    )*) => {
        $(impl BuiltinFunctions {
            $( #[$attr] )*
            pub(crate) fn $name(&mut self, func: &mut Function) -> ir::FuncRef {
                self.load_builtin(func, BuiltinFunctionIndex::$name())
            }
        })*
    };
}
foreach_builtin_function!(declare_function_signatures);

/// Helper structure for creating a `Signature` for all builtins.
pub struct BuiltinFunctionSignatures {
    pointer_type: Type,
    host_call_conv: CallConv,
    wasm_call_conv: CallConv,
    argument_extension: ir::ArgumentExtension,
}

#[expect(clippy::unused_self, reason = "macro use")]
impl BuiltinFunctionSignatures {
    pub(crate) fn new(isa: &dyn TargetIsa) -> Self {
        Self {
            pointer_type: isa.pointer_type(),
            host_call_conv: CallConv::triple_default(isa.triple()),
            wasm_call_conv: CallConv::Tail,
            argument_extension: isa.default_argument_extension(),
        }
    }

    fn vmctx(&self) -> AbiParam {
        AbiParam::special(self.pointer_type, ArgumentPurpose::VMContext)
    }
    fn pointer(&self) -> AbiParam {
        AbiParam::new(self.pointer_type)
    }
    fn u32(&self) -> AbiParam {
        AbiParam::new(types::I32)
    }
    fn u64(&self) -> AbiParam {
        AbiParam::new(types::I64)
    }
    fn u8(&self) -> AbiParam {
        AbiParam::new(types::I8)
    }
    fn bool(&self) -> AbiParam {
        AbiParam::new(types::I8)
    }

    pub fn wasm_signature(&self, builtin: BuiltinFunctionIndex) -> Signature {
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
                    if _cur == builtin.index() {
                        return Signature {
                            params: vec![ $( self.$param() ),* ],
                            returns: vec![ $( self.$result() )? ],
                            call_conv: self.wasm_call_conv,
                        };
                    }
                    _cur += 1;
                )*
            };
        }

        foreach_builtin_function!(iter);

        unreachable!();
    }

    pub fn host_signature(&self, builtin: BuiltinFunctionIndex) -> Signature {
        let mut sig = self.wasm_signature(builtin);
        sig.call_conv = self.host_call_conv;

        // Once we're declaring the signature of a host function we must
        // respect the default ABI of the platform which is where argument
        // extension of params/results may come into play.
        for arg in sig.params.iter_mut().chain(sig.returns.iter_mut()) {
            if arg.value_type.is_int() {
                arg.extension = self.argument_extension;
            }
        }

        sig
    }
}

/// Return value of [`BuiltinFunctionIndex::trap_sentinel`].
pub enum TrapSentinel {
    /// A falsy or zero value indicates a trap.
    Falsy,
    /// The value `-2` indicates a trap (used for growth-related builtins).
    NegativeTwo,
    // /// The value `-1` indicates a trap .
    // NegativeOne,
    /// Any negative value indicates a trap.
    Negative,
}

impl BuiltinFunctionIndex {
    /// Describes the return value of this builtin and what represents a trap.
    ///
    /// Libcalls don't raise traps themselves and instead delegate to compilers
    /// to do so. This means that some return values of libcalls indicate a trap
    /// is happening and this is represented with sentinel values. This function
    /// returns the description of the sentinel value which indicates a trap, if
    /// any. If `None` is returned from this function then this builtin cannot
    /// generate a trap.
    #[allow(unreachable_code, unused_macro_rules, reason = "macro-generated code")]
    pub fn trap_sentinel(self) -> Option<TrapSentinel> {
        macro_rules! trap_sentinel {
            (
                $(
                    $( #[$attr:meta] )*
                    $name:ident( $( $pname:ident: $param:ident ),* ) $( -> $result:ident )?;
                )*
            ) => {{
                $(
                    $(#[$attr])*
                    if self == BuiltinFunctionIndex::$name() {
                        let mut _ret = None;
                        $(_ret = Some(trap_sentinel!(@get $name $result));)?
                        return _ret;
                    }
                )*

                None
            }};

            // Growth-related functions return -2 as a sentinel.
            (@get memory_grow pointer) => (TrapSentinel::NegativeTwo);
            (@get table_grow_func_ref pointer) => (TrapSentinel::NegativeTwo);
            // (@get table_grow_gc_ref pointer) => (TrapSentinel::NegativeTwo);

            // Atomics-related functions return a negative value indicating trap
            // indicate a trap.
            (@get memory_atomic_notify u64) => (TrapSentinel::Negative);
            (@get memory_atomic_wait32 u64) => (TrapSentinel::Negative);
            (@get memory_atomic_wait64 u64) => (TrapSentinel::Negative);

            // Bool-returning functions use `false` as an indicator of a trap.
            (@get $name:ident bool) => (TrapSentinel::Falsy);

            (@get $name:ident $ret:ident) => (
                compile_error!(concat!("no trap sentinel registered for ", stringify!($name)))
            )
        }

        foreach_builtin_function!(trap_sentinel)
    }
}
