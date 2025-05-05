# Boot Arguments

Boot arguments configure various aspects of the kernels behaviour. They read from the 
[`/chosen/bootargs`](https://devicetree-specification.readthedocs.io/en/stable/devicenodes.html#chosen-node) property of the
flattened device tree that is passed to the kernel by the previous stage bootloader.

The format is a simple `key=value;key=value;..` list of semicolon separated of key-value pairs.

## `log`

Allows configuring the verbosity and filtering of debug messages.

```sh
# Enable the most verbose logging messages
cargo xtask qemu profile/riscv64/qemu.toml -- --append "log=trace"
# A more reasonable configuration that keeps trace messages enabled, but silences the very spammy ones
cargo xtask qemu profile/riscv64/qemu.toml -- --append "log=trace,cranelift_codegen=off,ksharded_slab=off"
```

## `backtrace`

Allows configuring the verbosity of kernel panic backtraces. There are two possible values: `short` (default) and `full`.
`short` will print an abridged backtrace that omits frames related to the unwinding and panic machinery itself.

```sh
# To print shorter panic backtraces (the default)
cargo xtask qemu profile/riscv64/qemu.toml -- --append "backtrace=short"
# To print more verbose panic backtraces
cargo xtask qemu profile/riscv64/qemu.toml -- --append "backtrace=full"
```