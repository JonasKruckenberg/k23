# Adding a Third-Party Dependency

> Skeleton — bullets to expand into prose.

External crates flow through `reindeer`, which converts a Cargo manifest into buck2 rules. This is the **only** sanctioned way to introduce non-first-party code.

## TL;DR

1. add the crate to `third-party/Cargo.toml`
2. run `just buckify`
3. depend on it from a first-party crate via `//third-party:<crate-name>`
4. commit `third-party/Cargo.toml`, `third-party/Cargo.lock`, and the (sometimes large) `third-party/BUCK` diff together

CI's `reindeer-clean` job will fail if step 2 was forgotten.

## The fields you'll touch

- `third-party/Cargo.toml` — the master manifest reindeer reads
  - `[dependencies]` for plain crates
  - `default-features = false` is the norm — most of our deps need to be `no_std`-friendly
  - `features = [...]` only for what you actually need
  - mark optional with `optional = true` if the crate is only pulled in by some downstream feature
- `third-party/Cargo.lock` — auto-managed; commit it as-is
- `third-party/BUCK` — generated, large, do not hand-edit
- `third-party/fixups/<crate>/fixups.toml` — optional per-crate overrides for the rare cases where reindeer needs hints (build script behavior, env vars, conditional features). Look at existing examples (`getrandom`, `rustix`, `serde`) before writing one
- `third-party/deny.toml` — license allowlist; cargo-deny CI checks against this

## Workflow

1. **Add to manifest**
   ```toml
   # third-party/Cargo.toml
   [dependencies]
   foo = { version = "0.4", default-features = false, features = ["bar"] }
   ```
2. **Update `Cargo.lock`**
   ```sh
   nix develop . --command reindeer update
   ```
   *(or just `cargo update --manifest-path third-party/Cargo.toml -p foo`)*
3. **Regenerate buck rules**
   ```sh
   just buckify
   ```
4. **Use it in a `BUCK`**
   ```starlark
   deps = [
       "//third-party:foo",
       ...
   ]
   ```
5. **Verify**
   - `just check //path/to/consumer:target`
   - `just preflight //path/to/consumer:target`
   - if cargo-deny complains about the license, either pick a different crate or extend `third-party/deny.toml` (talk to maintainers first)

## When reindeer needs a fixup

- the crate has a custom build script that does code generation → may need `buildscript` overrides
- the crate gates code on env vars set at compile time → declare them in the fixup
- the crate has features whose `cfg(...)` reindeer can't infer → spell them out
- look at `third-party/fixups/getrandom/fixups.toml` for an env-var case, `third-party/fixups/serde/fixups.toml` for a feature case

## Updating an existing dep

- bump the version in `third-party/Cargo.toml`
- `nix develop . --command reindeer update`
- `just buckify`
- commit all three files together
- if the upgrade is breaking, expect to also update consumer code

## Removing a dep

- delete from `third-party/Cargo.toml`
- delete the corresponding `third-party/fixups/<crate>/` if any
- `just buckify`
- commit

## Git dependencies

- prefer registry releases — git deps complicate caching and reproducibility
- if unavoidable, pin a `branch` or `rev` in `Cargo.toml` and add the host to `third-party/deny.toml` `[sources] allow-git`
- examples we currently keep: `JonasKruckenberg/wasmtime` (cranelift no_std fork)

## Why all this, vs `cargo add`?

- buck2 needs the dependency graph in its own format; `Cargo.toml` is the input, `BUCK` is the output
- regenerating from a single source of truth keeps the `BUCK` file deterministic across machines
- the alternative — hand-rolling rules — is a maintenance trap and skips license/auditing checks
