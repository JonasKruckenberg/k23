mod builtins;
mod compile;
mod engine;
mod errors;
pub mod instance;
// mod instantiate;
mod translate;
mod utils;
mod vmcontext;

pub use compile::compile_module;
pub use engine::Engine;
pub use errors::CompileError;
pub use vmcontext::VMContext;

/// Namespace corresponding to wasm functions, the index is the index of the
/// defined function that's being referenced.
pub const NS_WASM_FUNC: u32 = 0;

/// Namespace for builtin function trampolines. The index is the index of the
/// builtin that's being referenced.
pub const NS_WASM_BUILTIN: u32 = 1;

/// WebAssembly page sizes are defined to be 64KiB.
pub const WASM_PAGE_SIZE: u32 = 0x10000;

/// The number of pages (for 32-bit modules) we can have before we run out of
/// byte index space.
pub const WASM32_MAX_PAGES: u64 = 1 << 16;
/// The number of pages (for 64-bit modules) we can have before we run out of
/// byte index space.
pub const WASM64_MAX_PAGES: u64 = 1 << 48;

/// Trap code used for debug assertions we emit in our JIT code.
pub const DEBUG_ASSERT_TRAP_CODE: u16 = u16::MAX;

pub fn flow(wasm: &[u8]) {
    // 1. Global State Setup
    // let target_isa = todo!()
    // let engine = Engine::new(target_isa);

    // 2. Setup parsing & translation state
    // let features = WasmFeatures::default();
    // let mut validator = Validator::new_with_features(features);
    // let parser = cranelift_wasm::wasmparser::Parser::new(0);
    // let module_env = ModuleEnvironment::new(&mut validator)

    // 3. Perform WASM -> Cranelift IR translation
    // let translation = module_env.translate(parser, wasm)?;

    // 4. collect all the necessary context and gather the functions that need compiling
    // let compile_inputs = CompileInputs::from_module(&module, &types, function_body_inputs);

    // 5. compile functions to machine code
    // let unlinked_compile_outputs = compile_inputs.compile(&engine, &module)?;

    // 6. link functions & resolve relocations
    // let compiled_module = unlinked_compile_outputs.link();

    // =========================== Instantiate ===========================
    // WITH `CompiledModule` AND `imports: &[Extern]`

    // 0. Map builtins
    //      - map Builtins (builtins are precompiled and in a separate .wasm_builtins section of the kernel)

    // 1. Allocate space
    //      - for CodeMemory
    //      - for VMContext
    //      - for Stack
    //      - for each table
    //      - for each memory
    // result -> Instance

    // 2. Initialize CodeMemory final relocations (incl linking to builtins) & publish

    // 3. Initialize VMContext
    //  - set magic value
    //  - init tables (by using VMTableDefinition from Instance)
    //  - init memories (by using )
    //  - init memories
    //      - insert VMMemoryDefinition for every not-shared, not-imported memory
    //      - insert *mut VMMemoryDefinition for every not-shared, not-imported memory
    //      - insert *mut VMMemoryDefinition for every not-imported, shared memory
    //  - init globals from const inits
    //  - TODO funcrefs??
    //  - init imports
    //      - copy from imports.functions
    //      - copy from imports.tables
    //      - copy from imports.memories
    //      - copy from imports.globals
    //  - set stack limit
    //  - dont init last_wasm_exit_fp, last_wasm_exit_pc, or last_wasm_entry_sp bc zero initialization

    // 3. Initialize tables from const init exprs
    // 4. Initialize memories from const init exprs
    // 5. IF present => run start function
    // 6.
}
