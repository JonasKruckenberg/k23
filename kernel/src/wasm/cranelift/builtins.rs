// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![expect(unused, reason = "TODO")]

use crate::wasm::builtins::BuiltinFunctionIndex;
use crate::wasm::compile::NS_BUILTIN;
use alloc::vec;
use cranelift_codegen::ir::{self, AbiParam, ArgumentPurpose, Function, Signature, Type, types};
use cranelift_codegen::isa::{CallConv, TargetIsa};
use cranelift_entity::EntityRef;

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
        let signature = func.import_signature(self.types.signature(index));
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
crate::foreach_builtin_function!(declare_function_signatures);

/// Helper structure for creating a `Signature` for all builtins.
pub struct BuiltinFunctionSignatures {
    pointer_type: Type,
    call_conv: CallConv,
}

#[expect(clippy::unused_self, reason = "macro use")]
impl BuiltinFunctionSignatures {
    pub(crate) fn new(isa: &dyn TargetIsa) -> Self {
        Self {
            pointer_type: isa.pointer_type(),
            call_conv: CallConv::triple_default(isa.triple()),
        }
    }

    fn vmctx(&self) -> AbiParam {
        AbiParam::special(self.pointer_type, ArgumentPurpose::VMContext)
    }

    fn u8(&self) -> AbiParam {
        AbiParam::new(types::I8)
    }
    fn i32(&self) -> AbiParam {
        AbiParam::new(types::I32)
    }
    fn i64(&self) -> AbiParam {
        AbiParam::new(types::I64)
    }

    #[expect(clippy::no_effect_underscore_binding, reason = "empty builtins")]
    pub(crate) fn signature(&self, builtin: BuiltinFunctionIndex) -> Signature {
        let mut _cur = 0usize;
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
                            call_conv: self.call_conv,
                        };
                    }
                    _cur += 1;
                )*
            };
        }

        crate::foreach_builtin_function!(iter);

        unreachable!();
    }
}
