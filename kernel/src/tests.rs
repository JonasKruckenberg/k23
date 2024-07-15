#[cfg(test)]
mod tests {
    use crate::runtime::{Engine, Linker, Module, Store};
    use cranelift_codegen::settings::Configurable;

    static FIXTURES: &[&[u8]] = &[
        include_bytes!("../tests/fib-cpp.wasm"),
        include_bytes!("../tests/fib-porffor.wasm"),
        include_bytes!("../tests/fib-rs-debug.wasm"),
        include_bytes!("../tests/fib-rs-release.wasm"),
    ];

    #[ktest::test]
    fn compile() -> ktest::TestResult {
        for wasm in FIXTURES {
            let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
            let mut b = cranelift_codegen::settings::builder();
            b.set("opt_level", "speed_and_size").unwrap();

            let target_isa = isa_builder
                .finish(cranelift_codegen::settings::Flags::new(b))
                .unwrap();

            let engine = Engine::new(target_isa);

            let mut store = Store::new(0);

            let module = Module::from_binary(&engine, &store, wasm);
            log::debug!("{module:#?}");

            let linker = Linker::new();
            let instance = linker.instantiate(&mut store, &module);
            instance.debug_print_vmctx(&store);
        }

        Ok(())
    }
}
