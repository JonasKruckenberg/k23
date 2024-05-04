//! # Evolution
//! 1. Bytes - raw user provided input (HOST)
//! 2. Module - parsed, validated & translated (HOST)
//! 3. CompiledModule - functions compiled to machine code, debug info, linked (in ELF format) (Owned HOST, pointers into GUEST)
//! 5. InstancePre - functions compiled, linked + debug info, const & table & data inits done, relocations applied, start func ran
//! 6. Instance
//!
//!
//!
//! # Flow
//!
//! =============  Compilation  =============
//! 1. `let rt = Runtime::new()`
//! 2. `let store = Store::new()` with asid from engine
//! 3. `let translation = Module::parse()` with wasm bytes from user
//! 4. `let vmctx = store.allocate()` allocate space for the VMContext
//! 5. for each function in `module`
//!     1. `let code_buf = store.allocate()` allocate memory for function
//!     2. `let compiled_func = rt.compiler.compile_func()`
//!     3. Insert CompiledFunc into VMContext
//! 6. resolve relocations
//! ============= Instantiation =============
//! 7. for each global in `module`
//!     1. Eval const expression
//!     2. Insert global data into VMContext
//! 8. for each funcref in

mod builtins;
mod code_memory;
mod compile;
mod compiler;
mod const_expr;
mod cranelift;
mod engine;
mod module;
mod store;
mod wasm2ir;

struct ObjectBuilder {}
