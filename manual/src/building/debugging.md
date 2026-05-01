# Debugging k23

> **Note:** the debugging story for k23 is very much a work in progress. The flow described below works, but it is
> rough around the edges and the steps are still mostly manual. Improvements — better launch ergonomics, pretty
> printers, an LLDB/GDB init script bundled with the repo — are very welcome; if you'd like to help, please reach out.

The rest of this guide assumes you are using LLDB, but the same principles apply to GDB and "command translation guides"
are available online.

## Debug logging

The kernel uses the [`tracing`](https://docs.rs/tracing/latest/tracing/) to produce the kernel debuglog  (as well as span information).
In order to emit messages to this debuglog you should use the following macros:

```rust
fn function() {
    tracing::trace!("Trace");
    tracing::debug!("Debug");
    tracing::info!("Info");
    tracing::warn!("Warn");
    tracing::error!("Error");
}
```

Note that the `log` macros will work as well, but that support only exists to capture output from 3rd party crates, kernel
code should generally use `tracing`.

The debuglog will be printing to the semihosting STDOUT at the moment.

### Filtering

By default, the debuglog will only print messages of severity `DEBUG` and higher (i.e. `DEBUG`, `INFO`, `WARN`, and `ERROR`),
but this can be filtered and configured using the same syntax as `tracing`s [`EnvFilter`](https://docs.rs/tracing-subscriber/0.3.19/tracing_subscriber/filter/struct.EnvFilter.html),
by passing the `log` boot argument.

For example, to enable all levels you can pass this directive in the `log` boot argument:

```sh
just run //sys:k23-qemu-riscv64 -- --append "log=trace"
```

A more reasonable configuration that omits the quite verbose output from cranelift but otherwise keeps the trace logging:

```sh
just run //sys:k23-qemu-riscv64 -- --append "log=trace,cranelift_codegen=off"
```

### Attaching to the Kernel

There is no convenience flag for this yet — you wire it up by hand using QEMU's gdbstub. Forward `-s -S` to QEMU to
have it expose a gdb server on `localhost:1234` and halt the CPU at startup:

```sh
just run //sys:k23-qemu-riscv64 -- -s -S
```

The equivalent `buck2` invocation is `buck2 run //sys:k23-qemu-riscv64 -- -s -S`.

In a second terminal, ask Buck2 for the path to the freshly built kernel ELF and launch LLDB against it:

```sh
rust-lldb "$(buck2 build --show-output //sys/kernel:kernel | awk '{print $2}')"

# In LLDB
gdb-remote localhost:1234
```

### Catching Panics

Quite often, you will need to stop the kernel when a panic occurs, to inspect the state of the system. For this you can
set a breakpoint on the `rust_panic` symbol which is a special unmangled function for exactly this purpose (this
technique mirrors Rusts `std` library and is implemented in the `panic-unwind`
crate [here](../../../lib/panic-unwind/src/lib.rs)).

Using LLDB you can set a breakpoint with the following command:

```
b rust_panic
```

and then use e.g. the `bt` command to print a backtrace.

### Pretty Printing

To make debugging easier, you can add pretty printers for the `mem_core::PhysicalAddress` and `mem_core::VirtualAddress`
types. This can be done by through the following commands in LLDB:

```
type summary add --summary-string "mem_core::PhysicalAddress(${var.0%x})" mem_core::PhysicalAddress
type summary add --summary-string "mem_core::VirtualAddress(${var.0%x})" mem_core::VirtualAddress
```