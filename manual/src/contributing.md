# Contributing

Thanks for considering contributing to the project! This document is a collection of guidelines and tips to help you get started.

## Architecture

k23 can be broken down into 3 main components: The [bootloader](https://github.com/JonasKruckenberg/k23/blob/07322361bd99c04d8a6866fd8a5c565584393222/loader) that is responsible for loading the kernel, the [kernel itself](https://github.com/JonasKruckenberg/k23/tree/07322361bd99c04d8a6866fd8a5c565584393222/kernel), which is the main operating system, and the [WASM runtime](https://github.com/JonasKruckenberg/k23/tree/07322361bd99c04d8a6866fd8a5c565584393222/kernel/src/runtime), which is responsible for running WebAssembly programs. The last two components are highly intertwined by design.

### Bootloader

The bootloader is responsible for loading the kernel, verifying its integrity, decompressing it and setting up the necessary environment. That means collecting earyl information about the system, setting up the stack for each hart, setting up the page tables, and finally jumping to the kernel's entry point.

The bootloader has to be generic over the payloads it accepts, since the kernel is not the only thing that can be loaded. When running tests, each test is compiled as a separate binary and ran in separate VMs. The bootloader has to be able to load these binaries as well.

For this, payloads can declare their entry points and a few options through the `loader_api` crates `#[entry]` macro. The bootloader then uses this information to set up the environment for the payload. This macro also enforces a type signature for the entry point, which means that payloads can completely forgo the usual assembly tramploines and just declare a Rust function as their entry point.

### Kernel

The kernel is relatively minimal at the moment, and as a microkernel will likely stay that way. Much of the kernels functions, such as memory management, syscalls etc. are implemented in the runtime. This leaves only the most basic functions in the kernel, such as interrupt handling, physical memory management and the like.

### WASM Runtime

The WASM runtime is the heart of k23, it is responsible for running WebAssembly programs. It is not a standalone crate, but implemented as part of the kernel since it is so core to the system. The runtime uses the `wasmparser` and `cranelift` crates to parse and compile the WASM programs.

Currently, the runtime is quite simple, it only supports the most basic WASM instructions and features.

TODO this section will expand with more info.

## Debugging

The rest of this guide assumes you are using LLDB, but the same principles apply to GDB and "command translation guides" are available online.

### Attaching to the Kernel

You can run the kernel with the `--debug` or `--dbg` (or `--gdb` for typos) flag to start the kernel in a paused state. You can then launch and attach to the kernel with LLDB using the following commands:

```sh
# use the path that `just run-riscv64` outputs as the "payload"
rust-lldb target/riscv64gc-unknown-none-elf/debug/kernel

# In LLDB
gdb-remote localhost:1234
```

### Catching Panics

Quite often, you will need to stop the kernel when a panic occurs, to inspect the state of the system. For this you can set a breakpoint on the `rust_panic` symbol which is a special unmangled function for exactly this purpose (this technique mirrors Rusts `std` library and is implemented in the `kstd` crate [here](https://github.com/JonasKruckenberg/k23/blob/07322361bd99c04d8a6866fd8a5c565584393222/libs/kstd/src/panicking.rs#L89)).

Using LLDB you can set a breakpoint with the following command:

```
b rust_panic
```

and then use e.g. the `bt` command to print a backtrace.

### Pretty Printing

To make debugging easier, you can add pretty printers for the `vmm::PhysicalAddress` and `vmm::VirtualAddress` types. This can be done by through the following commands in LLDB:

```
type summary add --summary-string "vmm::PhysicalAddress(${var.0%x})" vmm::PhysicalAddress
type summary add --summary-string "vmm::VirtualAddress(${var.0%x})" vmm::VirtualAddress
```
