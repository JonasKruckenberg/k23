// #[cfg(test)]
// mod compile_tests {
//     use crate::runtime::{Engine, Linker, Module, Store};
//     use cranelift_codegen::settings::Configurable;
//
//     static FIXTURES: &[&[u8]] = &[
//         include_bytes!("../tests/fib-cpp.wasm"),
//         include_bytes!("../tests/fib-porffor.wasm"),
//         include_bytes!("../tests/fib-rs-debug.wasm"),
//         include_bytes!("../tests/fib-rs-release.wasm"),
//     ];
//
//     #[ktest::test]
//     fn compile() {
//         for wasm in FIXTURES {
//             let isa_builder = cranelift_codegen::isa::lookup(target_lexicon::HOST).unwrap();
//             let mut b = cranelift_codegen::settings::builder();
//             b.set("opt_level", "speed_and_size").unwrap();
//
//             let target_isa = isa_builder
//                 .finish(cranelift_codegen::settings::Flags::new(b))
//                 .unwrap();
//
//             let engine = Engine::new(target_isa);
//
//             let mut store = Store::new(0);
//
//             let module = Module::from_binary(&engine, &store, wasm);
//             log::debug!("{module:#?}");
//
//             let linker = Linker::new();
//             let instance = linker.instantiate(&mut store, &module);
//             instance.debug_print_vmctx(&store);
//         }
//     }
// }

#[cfg(test)]
mod kstd_tests {
    use core::sync::atomic::{AtomicU8, Ordering};

    use kstd::sync::{LazyLock, Once, OnceLock};

    #[ktest::test]
    fn panic_in_test() {
        assert!(false);
    }

    #[ktest::test]
    fn kstd_once() {
        let once = Once::new();

        assert!(!once.is_completed());

        let mut called = false;
        once.call_once(|| {
            called = true;
        });
        assert!(called);

        let mut called_twice = false;
        once.call_once(|| {
            called_twice = true;
        });
        assert!(!called_twice);

        assert!(once.is_completed());
    }

    #[ktest::test]
    fn kstd_once_lock() {
        let lock = OnceLock::new();

        assert!(lock.get().is_none());

        let val = lock.get_or_init(|| 42);
        assert_eq!(*val, 42);

        assert_eq!(lock.get(), Some(&42));
    }

    #[ktest::test]
    fn kstd_lazy_lock() {
        let mut called = AtomicU8::default();
        let lock = LazyLock::new(|| {
            called.fetch_add(1, Ordering::Relaxed);
            42
        });
        assert_eq!(called.load(Ordering::Acquire), 0);
        assert_eq!(*lock, 42);
        assert_eq!(called.load(Ordering::Acquire), 1);
        assert_eq!(*lock, 42);
        assert_eq!(called.load(Ordering::Acquire), 1);
    }
}
