# Running Checks Across Architectures

`just preflight` runs one CI lane locally. Without `platform=`, that's the
host lane: lint, check, unittests, miri, loom, selftests, and the audit /
license group. With `platform=X`, it's the X lane: lint and check at X,
plus the universal audit group. Unit, miri, and loom tests are host-only
by construction (they declare `target_compatible_with = [host_configuration.os,
host_configuration.cpu]`) and `--skip-incompatible-targets` drops them
silently under `platform=X`. `selftests` always boots the riscv64 qemu
image; per-arch qemu_test targets aren't wired yet.

```sh
just preflight                                 # full host lane
just platform=//platforms:riscv64 preflight    # riscv64 lane
just platform=//platforms:aarch64 preflight    # aarch64 lane
just platform=//platforms:x86_64  preflight    # x86_64 lane
```

The same `platform=` flag is accepted by `check`, `clippy`, `doc`,
`unittests`, `miri`, `loom`, and `benchmark` if you want to invoke a single
phase. CI matrixes `check`, `clippy`, and `selftests` across the supported
arches on every push; the host lane additionally runs unittests, miri,
loom, and the audit group.

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
