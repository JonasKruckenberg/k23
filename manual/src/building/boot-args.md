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

## `unstable_preload`

Allows loading base64-encoded wasm modules into a machine-local registry. This is useful to dynamically run programs 
without having to recompile the kernel.

Expects a `=` separated key value pair where the key is the module identifier that can be passed to the `unstable-eval` command and the value is a base64-encoded wasm module.

Example:

```sh
# base64-encoded fib module
cargo xtask qemu profile/riscv64/qemu.toml -- --append "unstable_preload=fib=AGFzbQEAAAABBgFgAX8BfwMCAQAEBQFwAQEBBQMBAAEGCAF/AUGAiAQLBxACBm1lbW9yeQIAA2ZpYgAACsoBAccBARV/IwAhAUEgIQIgASACayEDQQAhBEEBIQUgAyAANgIcIAMgBDYCFCADIAU2AhAgAyAENgIMAkADQCADKAIMIQYgAygCHCEHIAYhCCAHIQkgCCAJSCEKQQEhCyAKIAtxIQwgDEUNASADKAIUIQ0gAyANNgIYIAMoAhAhDiADIA42AhQgAygCGCEPIAMoAhAhECAQIA9qIREgAyARNgIQIAMoAgwhEkEBIRMgEiATaiEUIAMgFDYCDAwACwsgAygCECEVIBUPCw=="
```

use at runtime:

```sh
unstable-eval fib fib 42
# will print "results: [I32(433494437)]"
```
