# Running Checks Across Architectures

`just preflight` runs every lane CI runs: hygiene, clippy on all three
arches, then unittests, miri, loom and the qemu selftests, then the audit /
license group. Unit, miri, and loom tests are host-only by construction
(they declare `target_compatible_with = [host_configuration.os,
host_configuration.cpu]`) and `--skip-incompatible-targets` drops them
silently on the arch lanes. `selftests` always boots the riscv64 qemu
image; per-arch qemu_test targets aren't wired yet.

By default it only covers the targets your changes affect, so it is cheap
enough to run before every push. Pass targets explicitly to override:

```sh
just preflight                        # everything your changes affect
just preflight //...                  # the whole workspace
just preflight //lib/mycrate:mycrate  # one crate
```

`just changed-targets` prints the affected set on its own if you want to see
what preflight will act on. Individual phases (`check`, `clippy`, `doc`,
`unittests`, `miri`, `loom`, `benchmark`) take the same target list plus a
`platform=X` flag if you want to invoke just one.

A crate that's intrinsically host-only (test runners, benches, fuzzers,
host tooling like `mkdisk-img`) declares this with `target_compatible_with`:

```python
load("@prelude//platforms:defs.bzl", "host_configuration")

rust_test(
    name = "my_unittests",
    srcs = glob(["src/**/*.rs"]),
    target_compatible_with = [host_configuration.os, host_configuration.cpu],
    visibility = ["PUBLIC"],
)
```

The `rust_benchmark`, `rust_loom_test`, and `rust_fuzz` wrappers in
`build/{bench,loom,fuzz}.bzl` apply this automatically. A crate that's
arch-locked (e.g. `lib/riscv`, the kernel, the loader) declares the
inverse:

```python
target_compatible_with = ["prelude//cpu/constraints:riscv64"],
```
