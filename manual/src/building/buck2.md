# The buck2 build system

## Why buck2

k23 is not a typical Rust project. We produce many different artifacts:

- The **kernel** build for a custom Rust target and with from-source-rebuilt `core` and `alloc` crates
- The **loader** binary that is built with different Rust flags and for a different Rust target
- The full disk image(s) that is a combination of both binaries, with an initial ramdisk and possibly drivers/apps
- Additionally we also have many different kinds of tests (unittest, fuzz, loom, wasm-spec, on-target tests, etc) that all require different modes and apply only to subsets of libraries.

Cargo's build model (one `--target`, one profile, one feature resolution per workspace) is unfortunately not well equipped to handle this. k23 needs a build system that is flexible, can deal with the same source node appearing multiple times with different configuration, and where post-processing steps are easy to express.

That's what buck2 gives us. Complex tooling (rust, c++, mdbook, qemu, python) is wired in as ordinary build rules. The build graph is hermetic and content-addressed. Buck2 can schedule much more optimal builds across the entire build graph and aggressively cache results along the way. Because buck2 isn't Rust-specific, a full "image" (kernel + loader + ramdisk + apps) and  QEMU runner can be declared as an elegant chaining of rules.

## High-level components

### Tree layout

```text
k23/
├── sys/             non-standalone subsystems (only make sense inside k23)
│   ├── loader/      bootloader binary
│   ├── kernel/      kernel binary
│   └── async/       kasync — the async runtime
├── lib/             standalone, potentially-publishable libraries
│                    (range-tree, wavltree, cpu-local, spin, fdt, …)
├── third-party/     reindeer-generated BUCK rules; the one source of truth
│                    for every non-first-party dep
├── tests/           wasm spec testsuite + handwritten .wast fixtures
├── platforms/       target platforms (riscv64, aarch64, x86_64) bundling
│                    constraint values
├── manual/          the mdbook you are reading (//manual:manual)
├── build/           the buck2 build infrastructure itself (see below)
├── fuzz/            running corpus (gitignored) + committed crash repros
│                    in fuzz/artifacts/
├── bench/           criterion baselines; gitignored; cached on main in CI
└── buck-out/        buck2's everything; gitignored; `buck2 clean` clears it
```


### Build infrastructure (`build/`)

Everything that defines *how* k23 is built (as opposed to *what* gets built) lives in `build/`:

```text
build/
├── BUCK               declares kcfg options, target JSON, and the named
│                      transitions (loader, kernel, rust_bootstrap, fuzz)
├── constraints/       constraint enums (opt-level, debuginfo, strip,
│                      rust-std, env, sanitizer)
├── toolchains/        toolchain rules (rust, cxx, qemu, mdbook, python, …)
│                      plus flake.bzl, which exposes nix-flake packages
├── targets/           Rust target-spec JSON files
├── transitions.bzl    the generic configuration-transition rule
├── kcfg.bzl           typed buckconfig wrapper + kcfg_docs rule that
│                      auto-generates the config reference in this manual
├── qemu.bzl           qemu_binary — wraps a kernel ELF into a QEMU command
├── bench.bzl          rust_benchmark macro (criterion)
└── fuzz.bzl           rust_fuzz macro (libfuzzer + persistent corpus)
```

## Cargo to buck2 cheat sheet

| Cargo Command | k23 Equivalent |
|---|---|
| `cargo check` / `cargo build` | `just check` |
| `cargo build -p foo` | `just check //lib/foo:foo` |
| `cargo test` / `cargo test -p foo` | `just unittests` / `just unittests //lib/foo:foo` |
| `cargo bench` | `just benchmark` |
| `cargo fuzz run target` | `just fuzz` |
| `cargo clippy` | `just clippy` |
| `cargo fmt --check` / `cargo fmt` | `just check-fmt` / `just fmt` |
| `cargo doc` | `just doc` |
| edit `Cargo.toml` / `cargo update` | edit `third-party/Cargo.toml`, then `just buckify` |
| `[package]` | `rust_library` / `rust_binary` in a `BUCK` file |
| `[dependencies]` | `deps = [...]` attribute of a `rust_library`/`rust_binary` rule |
| `[features]` | `features = [...]` attribute of a `rust_library`/`rust_binary` rule |
| profiles (`debug`/`release`) | constraints (`opt-level`/`debuginfo`/`strip`) + named modifier aliases in `PACKAGE` |
| `--target=…` | constraints (`prelude//cpu:riscv64`, …) bundled into a *platform* under `platforms/` |
| `RUSTFLAGS=-Cfoo` | `rustc_flags = ["-Cfoo"]` attribute of a `rust_library`/`rust_binary` rule |
| `cfg(test)` | a dedicated `rust_test` target; can carry its own deps |
| `cfg(loom)` | `rust_test` with `rustc_flags = ["--cfg=loom"]` and `labels = ["loom"]` |
