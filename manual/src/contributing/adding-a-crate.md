# Adding a Crate

> Skeleton — bullets to expand into prose.

## Decide where it lives

- standalone library, could plausibly be published or used outside k23 → `lib/<name>/`
- subsystem that only makes sense as part of k23 (kernel modules, loader internals, async runtime, …) → `sys/<name>/`
- proc-macro helper that supports a subsystem → co-locate with its consumer (e.g. `lib/test/macros/`)

## Skeleton

- `lib/<name>/Cargo.toml` — kept for editor/IDE awareness; not consumed by buck2
  - workspace inherits (`version.workspace = true`, `edition.workspace = true`, …)
- `lib/<name>/BUCK` — the buck2 rules; this is what actually drives the build
- `lib/<name>/src/lib.rs` (or `src/main.rs` for binaries)
- optional: `benches/`, `tests/`, `fuzz/`, `README.md`, `CHANGELOG.md`

## Minimal `BUCK` for a `no_std` library + host unit tests

```starlark
load("@prelude//platforms:defs.bzl", "host_configuration")

rust_library(
    name = "mycrate",
    srcs = glob(["**/*.rs"]),
    deps = [
        "//lib/util:util",
        "//third-party:cfg-if",
    ],
    visibility = ["PUBLIC"],
    tests = [":mycrate_unittests"],
)

rust_test(
    name = "mycrate_unittests",
    srcs = glob(["**/*.rs"]),
    deps = [
        "//lib/util:util",
        "//third-party:cfg-if",
        "//third-party:proptest",  # or whatever the tests need
    ],
    visibility = ["PUBLIC"],
    target_compatible_with = [host_configuration.cpu, host_configuration.os],
)
```

- `name = "mycrate"` — also the buck target name (`//lib/mycrate:mycrate`); convention is to match the crate dir
- `glob(["**/*.rs"])` — picks up any new source file automatically
- `visibility = ["PUBLIC"]` — let other crates depend on it; tighten if it's strictly internal
- `tests = [...]` — wires `:mycrate_unittests` so `just unittests //lib/mycrate:mycrate` finds it
- **always** put `target_compatible_with = [host_configuration.cpu, host_configuration.os]` on `rust_test`/`rust_benchmark` — host-only by definition

## Pull it into a consumer

- add the buck path to the consumer's `deps`:
  ```starlark
  deps = [
      "//lib/mycrate:mycrate",
      ...
  ]
  ```
- if you also bumped `Cargo.toml`, that change is for the IDE only — buck2 ignores it

## Verify

- `just check //lib/mycrate:mycrate` — quick `cargo check`-equivalent
- `just unittests //lib/mycrate:mycrate` — run the new tests
- `just preflight //lib/mycrate:mycrate` — full lint+miri sweep
- `just rust-project` — re-emit `rust-project.json` so rust-analyzer sees the new crate

## Conventions worth following

- transitions in *consumers* propagate down — your library does **not** need to declare `incoming_transition`
- if the library has riscv-only code paths, gate them with `select({"prelude//cpu/constraints:riscv64": ["//lib/riscv:riscv"], "DEFAULT": []})` in `deps`
- if it has kernel-only behavior, gate features the same way: `features = select({"constraints//:env[kernel]": ["thread-local"], "DEFAULT": []})`
- if it must compile under `std` for tests but `no_std` for production, that's automatic — `rust_test` runs on host
- proc-macros: set `proc_macro = True` on `rust_library` (see `lib/test/BUCK` for an example)

## Removing a crate

- delete the directory
- remove buck deps from any consumer's `deps`
- remove from any `Cargo.toml` workspace member list (the workspace `Cargo.toml` is not actively used, but keep it tidy)
- regenerate `rust-project.json`
