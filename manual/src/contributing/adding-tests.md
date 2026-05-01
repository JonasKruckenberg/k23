# Adding Tests

> Skeleton — bullets to expand into prose.

k23 has four flavors of test, each with its own buck2 rule and its own `just` recipe.

## Unit tests (`rust_test`)

- write `#[test]` functions inline (`#[cfg(test)] mod tests`) or in `tests/` files
- declare in the crate's `BUCK`:
  ```starlark
  rust_test(
      name = "mycrate_unittests",
      srcs = glob(["**/*.rs"]),
      deps = [...],
      visibility = ["PUBLIC"],
      target_compatible_with = [host_configuration.cpu, host_configuration.os],
  )
  ```
- wire it via `tests = [":mycrate_unittests"]` on the library target
- run: `just unittests //lib/mycrate:mycrate` (or `just unittests` for the whole workspace)
- runs under miri automatically when invoked with `just miri`

## Loom tests (concurrency model checking)

- write tests gated on `#[cfg(loom)]`
- declare a *separate* `rust_test` target with the loom deps:
  ```starlark
  rust_test(
      name = "mycrate_loom_tests",
      srcs = glob(["**/*.rs"]),
      crate = "mycrate",
      rustc_flags = ["--cfg=loom"],
      modifiers = ["constraints//:opt-level[3]"],
      labels = ["loom"],
      env = {
          "LOOM_LOG": "kasync=trace,debug",
          "LOOM_MAX_PREEMPTIONS": "2",
          "LOOM_LOCATION": "true",
      },
      deps = [..., "//third-party:loom"],
      target_compatible_with = [host_configuration.cpu, host_configuration.os],
  )
  ```
- the `labels = ["loom"]` tag is what `just loom` filters on
- run: `just loom //lib/mycrate:mycrate`
- example: `lib/spin/BUCK`

## Fuzz tests

- one fuzz target per fuzzable surface; live under `<crate>/fuzz/<name>.rs`
- file contents start with `#![no_main]` and end in `fuzz_target!(|input: T| { ... })`
- declare with our `rust_fuzz` macro:
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
- the `fuzz` transition pins host + asan + opt3 + debuginfo[full] automatically
- run locally: `just fuzz_args='--test-arg=-max_total_time=60' fuzz //lib/mycrate:mycrate_fuzz`
- corpus and crash repros:
  - `fuzz/corpus/<name>/` — running corpus, gitignored, persisted via CI cache
  - `fuzz/artifacts/<name>/` — committed crash repros; replayed on every run as permanent regression tests
- when CI finds a crash, copy the file from the uploaded `fuzz-artifacts` bundle into `fuzz/artifacts/<name>/` and commit
- example: `lib/range-tree/fuzz/range_tree.rs` + `lib/range-tree/BUCK`

## Benchmarks

- live under `<crate>/benches/<name>.rs`, criterion-style
- declare with our `rust_benchmark` macro:
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
      target_compatible_with = [host_configuration.cpu, host_configuration.os],
  )
  ```
- defaults to `opt-level[3] + debuginfo[line-tables-only] + strip[debuginfo]` via `_DEFAULT_MODIFIERS`
- run locally: `just benchmark //lib/mycrate:mycrate_benchmarks`
- baselines live in `bench/` (gitignored); CI on main caches this so PRs compare against trunk
- examples: `lib/range-tree/benches/comparisons.rs`, `sys/async/benches/spawn.rs`

## Wasm spec tests

- `tests/testsuite/` is the upstream spec testsuite (git submodule)
- `tests/*.wast` are small handwritten regression fixtures (not from the testsuite)
- the kernel's `wast_tests!` macro in `sys/kernel/src/tests/spectest.rs` is where individual `.wast` files get enabled
- run via the kernel test harness: `just run //sys:k23-qemu-riscv64`
- (re-enabling the full spec testsuite is tracked separately — most entries are currently commented out)

## Picking the right flavor

- pure logic, deterministic → unit
- concurrency invariants → loom
- input-driven correctness over a wide input space → fuzz
- performance regressions → bench
- WASM language conformance → wast
