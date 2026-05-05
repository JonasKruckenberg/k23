# Boot Arguments

Boot arguments configure various aspects of the kernels behaviour. They read from the 
[`/chosen/bootargs`](https://devicetree-specification.readthedocs.io/en/stable/devicenodes.html#chosen-node) property of the
flattened device tree that is passed to the kernel by the previous stage bootloader.

The format is a simple `key=value;key=value;..` list of semicolon separated of key-value pairs.

## `log`

Allows configuring the verbosity and filtering of debug messages.

```sh
# Enable the most verbose logging messages
just run //sys:k23-qemu-riscv64 -- --append "log=trace"
# A more reasonable configuration that keeps trace messages enabled, but silences the very spammy ones
just run //sys:k23-qemu-riscv64 -- --append "log=trace,cranelift_codegen=off,sharded_slab=off"
```

The underlying `buck2` invocation is `buck2 run //sys:k23-qemu-riscv64 -- --append "..."`; everything after `--` is forwarded to QEMU.

## `backtrace`

Allows configuring the verbosity of kernel panic backtraces. There are two possible values: `short` (default) and `full`.
`short` will print an abridged backtrace that omits frames related to the unwinding and panic machinery itself.

```sh
# To print shorter panic backtraces (the default)
just run //sys:k23-qemu-riscv64 -- --append "backtrace=short"
# To print more verbose panic backtraces
just run //sys:k23-qemu-riscv64 -- --append "backtrace=full"
```