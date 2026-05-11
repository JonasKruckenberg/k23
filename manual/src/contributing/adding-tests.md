# Adding Tests

k23 has quite a number of test flavors, each with its own usecase.

## Unit tests (`rust_test`)

The most straightforward and common kind of test. These are just regular Rust unit tests that you write either inside the modules directly or in `test/` files. They use the regular rust `#[test]` macro annotated functions:

```rust
#[test]
fn foo() {
    assert!(true);
}
```

You declare them in the crate's BUCK file with the `rust_test` rule:

```starlark
load("@prelude//platforms:defs.bzl", "host_configuration")

rust_test(
    name = "mycrate_unittests",
    srcs = glob(["**/*.rs"]),
    deps = [...],
    target_compatible_with = [host_configuration.os, host_configuration.cpu],
    visibility = ["PUBLIC"],
)
```

`target_compatible_with` marks the test as host-only so cross-arch preflight
lanes (`just platform=//platforms:riscv64 …`) skip it via
`--skip-incompatible-targets` instead of trying to compile a `std`-using
test for a kernel target.

Lastly, don't forget to add the test to the crates' `tests` array! The test runner will not pick up on your tests
otherwise!

`just unittests` or `just unittests //lib/mycrate:mycrate` to run the tests.
`just miri` will automatically run the tests under [miri] as well.

## Loom tests (concurrency model checking)

[Loom][loom] is a very useful tool for checking concurrent and asynchronous code. It will explore many possible
concurrent executions of your code to find deadlocks, panics, race conditions and more. If your crate
touches anything concurrency related, you must absolutely add loom tests.

You declare them using the `rust_loom_test` rule:

```starlark
rust_loom_test(
    name = "mycrate_loom_tests",
    srcs = glob(["**/*.rs"]),
    deps = [..., "//third-party:loom"],
)
```

`rust_loom_test` automatically sets the correct compiler flags (`--cfg=loom` and others) and makes the loom tests visible to the build system. Run the loom tests with ``just loom //lib/mycrate:mycrate`.

See `lib/spin/BUCK` for a complete example.

## Fuzz tests

[Fuzz tests][libfuzzer] drive a function with random inputs to find non-obvious bugs. Any crate (especially parsers or data structures) that deal with user input should have a fuzz testing suite. Each lives under `<crate>/fuzz/<name>.rs`.

Declare the target in the crate's `BUCK` file using our `rust_fuzz` rule:

```starlark
load("//build:fuzz.bzl", "rust_fuzz")

rust_fuzz(
    name = "mycrate_fuzz",
    srcs = ["./fuzz/myfuzz.rs"],
    crate_root = "./fuzz/myfuzz.rs",
    deps = [
        ":mycrate",
        "//third-party:libfuzzer-sys",
        "//third-party:arbitrary",
    ],
    visibility = ["PUBLIC"],
)
```

The `rust_fuzz`  rule automatically sets the correct compiler flags and makes the fuzz tests visible to the build system. Fuzz targets use [`libfuzzer-sys`][libfuzzer-sys] for the harness and typically derive structured inputs with [`arbitrary`][arbitrary]. Run the fuzz tests with `just fuzz //lib/mycrate:mycrate_fuzz`. You can pass arguments such as the max time through the named `fuzz_args` argument `just fuzz_args='--test-arg=-max_total_time=60' fuzz //lib/mycrate:mycrate_fuzz`.

Fuzz tests produce two directories in the project root. 
- `fuzz/corpus/` is the running corpus. Persists exploration state between runs and makes them more useful.
- `fuzz/artifacts/` holds crashes the fuzz test found. Commit these so in the future we run them as regression tests. When CI finds a crash, copy the file from the uploaded `fuzz-artifacts` bundle into `fuzz/artifacts/<name>/` and commit it.

See `lib/range-tree/fuzz/range_tree.rs` and `lib/range-tree/BUCK` for a complete example.

## Benchmarks

Benchmarks measure performance and catch regressions. You should probably add a benchmark for _any_ library that is not a build dependency only. Each benchmark lives under `<crate>/benches/<name>.rs` and is written using [criterion].

Declare the benchmark in the crate's `BUCK` file using our `rust_benchmark` rule:

```starlark
load("//build:bench.bzl", "rust_benchmark")

rust_benchmark(
    name = "mycrate_benchmarks",
    srcs = ["./benches/whatever.rs"],
    crate_root = "./benches/whatever.rs",
    deps = [
        ":mycrate",
        "//third-party:criterion",
    ],
    visibility = ["PUBLIC"],
)
```

The `rust_benchmark` rule sets compiler flags automatically (`opt-level[3]`, `debuginfo[line-tables-only]`, `strip[debuginfo]`), marks the benchmark host-only via `target_compatible_with`, and makes it visible to the build system. Run it with `just benchmark //lib/mycrate:mycrate_benchmarks`.

Benchmarks produce a `bench/` directory in the project root holding baselines and reports.

See `lib/range-tree/benches/comparisons.rs` and `sys/async/benches/spawn.rs` for complete examples.

## Wasm tests

Wasm tests exercise the kernel's [WebAssembly] engine end-to-end. They are all written in the [wast] language — a superset of the WebAssembly text format that adds assertions like `assert_return` and `assert_trap` for declaring expected outcomes. `.wast` fixtures live under `tests/`; the `//:wast_tests` filegroup ships them into the kernel test binary under `wast/`.

Wasm tests are registered through the `wast_tests!` macro defined in `sys/kernel/src/tests/wast.rs` and invoked from `spectest.rs` / `smoke.rs`:

```rust
wast_tests!(
    fib "../../wast/tests/fib.wast",
    trap "../../wast/tests/trap.wast",
    // ...
);
```

Each entry pairs a test name with a path to a `.wast` file. The macro generates a kernel test for each entry; adding a new test is a matter of dropping the file into `tests/` and listing it in the macro. The paths above are rewritten by the kernel BUCK's `mapped_srcs = {"//:wast_tests": "wast"}`, so they're relative to that mapped location, not to the source tree.

Run them via the kernel test harness — `just selftests` boots `//sys:k23-qemu-riscv64-tests` under QEMU.

See `tests/fib.wast` and `tests/trap.wast` for complete examples.

[miri]: https://github.com/rust-lang/miri
[loom]: https://github.com/tokio-rs/loom
[libfuzzer]: https://llvm.org/docs/LibFuzzer.html
[libfuzzer-sys]: https://github.com/rust-fuzz/libfuzzer
[arbitrary]: https://github.com/rust-fuzz/arbitrary
[criterion]: https://github.com/bheisler/criterion.rs
[WebAssembly]: https://webassembly.org/
[wast]: https://webassembly.github.io/spec/core/text/index.html
