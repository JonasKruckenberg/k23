# Adding a Third-Party Dependency

Even though we use [buck2] and not [Cargo] to build k23 we nonetheless use 3rd party crates from [crates.io]. There are plenty of high-quality legitimately useful libraries available and we want to use them.

The tight integration between crates.io and Cargo (a good thing!) requires a bit of finagling which we do using [`reindeer`][reindeer] maintained by meta. It takes a `Cargo.toml` file, resolves all the crates and generates a `BUCK` file that lets us reference these
crates from throughout our buck2 project.

## TL;DR

1. add the crate to `third-party/Cargo.toml`
2. run `reindeer buckify`
3. depend on it from a first-party crate via `//third-party:<crate-name>`
4. commit `third-party/Cargo.toml`, `third-party/Cargo.lock`, and `third-party/BUCK`

The `reindeer-clean` job will complain if the Cargo.toml and BUCK file are out of sync.

## The fields you'll touch

- `third-party/Cargo.toml` — the master manifest reindeer reads
  - `[dependencies]` for plain crates
  - `default-features = false` is the norm — most of our deps need to be `no_std`-friendly
  - `features = [...]` only for what you actually need
  - mark optional with `optional = true` if the crate is only pulled in by some downstream feature
- `third-party/Cargo.lock` — auto-managed; commit it as-is
- `third-party/BUCK` — generated, large, do not hand-edit
- `third-party/fixups/<crate>/fixups.toml` — optional per-crate overrides for the rare cases where reindeer needs hints (build script behavior, env vars, conditional features). Look at existing examples (`getrandom`, `rustix`, `serde`) before writing one
- `third-party/deny.toml` — license allowlist; [cargo-deny] CI checks against this

## Adding a crate

1. **Add to manifest**
  ```toml
  # third-party/Cargo.toml
  [dependencies]
  foo = { version = "0.4", default-features = false, features = ["bar"] }
  ```

  If you're adding a dev or build dependency (that will be run on the host and needs access to `std`) you should mark it EITHER as `optional = true` and add it to the `default` feature OR enable it's `std`-requiring feature in the `default` feature.

2. **Update `Cargo.lock`**
  ```sh
  reindeer update
  ```
  
  This will update the Cargo.lock file, reusing the Cargo/crates.io resolution logic. The updated lockfile is required by the next step.

3. **Regenerate buck rules**
  ```sh
  reindeer buckify
  ```
  
  This will read the lockfile and generate the `third-party/BUCK` file. This file contains buck2 target declarations corresponding to the dependencies you added in step 1. Note that these buck2 targets directly fetch the libraries from crates.io, so you dont actually need Cargo installed _at all_.

4. **Use it in a `BUCK`**
  ```starlark
  deps = [
    "//third-party:foo",
    ...
  ]
  ```
  
  You can then reference 3rd party libraries by their buck2 path. The name of the target is the same as in the Cargo.toml manifest.
  
5. **Verify**

  To verify your changes are correct, you may run `just check //path/to/consumer:target` or `just preflight //path/to/consumer:target` to run all checks.

  If [`cargo-deny`][cargo-deny] complains about the 3rd party crates' license you may extend `third-party/deny.toml` if the license is MIT compatible or pick a different crate (it's probably best to discuss this with the maintainers and community first in any case).

## Reindeer fixups

In some situations `reindeer` needs human help to correctly generate the dependency graph. These hints are called "fixups" and live in `third-party/fixups/<crate>/fixups.toml` files. You will need a fixup when:

- the crate has a required buildscript => `reindeer` does not build/run buildscripts by default. You have to explicitly opt-in by setting `buildscript.run = true`
- the crate reads env vars set at compile time => you may need to declare them manually or set `cargo_env = true` for the "common" cargo env vars
- the crate has complex `cfg(...)` dependencies or features => you will need to spell them out, see the [reindeer manual][reindeer-fixups] for help.

[buck2-fixups] is a community maintained list of fixups for common crates.io dependencies. It's always worth a look.

See `third-party/fixups/getrandom/fixups.toml` and `third-party/fixups/serde/fixups.toml` for examples of complex fixups.

## Updating an existing dependency

Updating an existing dep is as easy as bumping its version in `third-party/Cargo.toml` and running `reindeer update` followed by `reindeer buckify`.

## Removing a dependency

Removing a dependency means deleting it from `third-party/Cargo.toml`, deleting its corresponding `third-party/fixups/<crate>/fixups.toml` if present and run `reindeer buckify` to synchronize the `third-party/BUCK` file.

## Git dependencies

You should prefer crates.io releases since git dependencies complicate and slow down the build. But if its unavoidable,
pin a `branch` or `rev` in `Cargo.toml` and add the host to `third-party/deny.toml` `[sources] allow-git`.

For example, we currently pull in `JonasKruckenberg/wasmtime` (the [cranelift] no_std fork) as a git dependency.

[buck2]: https://buck2.build/
[Cargo]: https://doc.rust-lang.org/cargo/
[crates.io]: https://crates.io/
[reindeer]: https://github.com/facebookincubator/reindeer
[reindeer-fixups]: https://github.com/facebookincubator/reindeer/blob/main/docs/MANUAL.md#fixups
[buck2-fixups]: https://github.com/gilescope/buck2-fixups
[cargo-deny]: https://github.com/EmbarkStudios/cargo-deny
[cranelift]: https://cranelift.dev/
