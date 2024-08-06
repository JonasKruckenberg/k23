#[cfg(test)]
mod compile_tests {
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

#[cfg(test)]
mod kstd_tests {
    use core::sync::atomic::{AtomicU8, Ordering};

    use kstd::sync::{LazyLock, Once, OnceLock};

    #[ktest::test]
    fn kstd_once() -> ktest::TestResult {
        let once = Once::new();

        ktest::assert!(!once.is_completed());

        let mut called = false;
        once.call_once(|| {
            called = true;
        });
        ktest::assert!(called);

        let mut called_twice = false;
        once.call_once(|| {
            called_twice = true;
        });
        ktest::assert!(!called_twice);

        ktest::assert!(once.is_completed());

        Ok(())
    }

    #[ktest::test]
    fn kstd_once_lock() -> ktest::TestResult {
        let lock = OnceLock::new();

        ktest::assert!(lock.get().is_none());

        let val = lock.get_or_init(|| 42);
        ktest::assert_eq!(*val, 42);

        ktest::assert_eq!(lock.get(), Some(&42));

        Ok(())
    }

}
