use crate::wasm::builtins::BuiltinFunctionIndex;

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
            /// The actual array of builtin functions pointers, the address of this will be used as
            /// the value for the `CMContext::builtin_functions` field.
            pub const INIT: VMBuiltinFunctionsArray = VMBuiltinFunctionsArray {
                $(
                    $name: crate::wasm::vm::builtins::$name,
                )*
            };
        }
    };

    (@ty i32) => (u32);
    (@ty i64) => (u64);
    (@ty u8) => (u8);
    (@ty reference) => (u32);
    (@ty pointer) => (*mut u8);
    (@ty vmctx) => (*mut VMContext);
}

crate::foreach_builtin_function!(define_builtin_array);

const _: () = {
    assert!(
        size_of::<VMBuiltinFunctionsArray>()
            == size_of::<usize>()
                * (BuiltinFunctionIndex::builtin_functions_total_number() as usize)
    );
};
