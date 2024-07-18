#[cfg(test)]
mod tests {
    use crate::runtime::{Engine, Linker, Module, Store};
    use cranelift_codegen::settings::Configurable;
    use wast::WastDirective;

    fn build_and_run_wasm(wasm: &[u8]) -> ktest::TestResult {
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

        Ok(())
    }

    fn build_and_run_wast(wast: &str) -> ktest::TestResult {
        use wast::{
            parser::{self, ParseBuffer},
            Wast, Wat,
        };

        let buf = ParseBuffer::new(wast).unwrap();
        let module = parser::parse::<Wast>(&buf).unwrap();
        for dir in module.directives {
            if let WastDirective::Wat(mut wat) = dir {
                let wasm = wat.encode().unwrap();
                build_and_run_wasm(&wasm)?;
            }
        }

        Ok(())
    }

    macro_rules! wasm_test_case {
        ($name:ident, $fixture:expr) => {
            #[ktest::test]
            fn $name() -> ktest::TestResult {
                let bytes = include_bytes!($fixture);
                build_and_run_wasm(bytes)
            }
        };
    }

    macro_rules! wast_test_case {
        ($name:ident, $fixture:expr) => {
            #[ktest::test]
            fn $name() -> ktest::TestResult {
                let bytes = include_str!($fixture);
                build_and_run_wast(bytes)
            }
        };
    }

    ktest::for_each_fixture!("../tests/fib", wasm_test_case);
    ktest::for_each_fixture!("../tests/testsuite", wast_test_case);
}
