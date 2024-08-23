# Overview of k23's Architecture

k23 has 3 main components:
The [bootloader](https://github.com/JonasKruckenberg/k23/blob/07322361bd99c04d8a6866fd8a5c565584393222/loader) that is
responsible for loading the kernel,
the [kernel itself](https://github.com/JonasKruckenberg/k23/tree/07322361bd99c04d8a6866fd8a5c565584393222/kernel), which
is the main operating system, and
the [WASM runtime](https://github.com/JonasKruckenberg/k23/tree/07322361bd99c04d8a6866fd8a5c565584393222/kernel/src/runtime),
which is responsible for running WebAssembly programs. The last two components are highly intertwined by design.

### Bootloader

The bootloader is responsible for loading the kernel, verifying its integrity, decompressing it and setting up the
necessary environment. That means collecting earyl information about the system, setting up the stack for each hart,
setting up the page tables, and finally jumping to the kernel's entry point.

The bootloader has to be generic over the payloads it accepts, since the kernel is not the only thing that can be
loaded. When running tests, each test is compiled as a separate binary and ran in separate VMs. The bootloader has to be
able to load these binaries as well.

For this, payloads can declare their entry points and a few options through the `loader_api` crates `#[entry]` macro.
The bootloader then uses this information to set up the environment for the payload. This macro also enforces a type
signature for the entry point, which means that payloads can completely forgo the usual assembly tramploines and just
declare a Rust function as their entry point.

### Kernel

The kernel is relatively minimal at the moment, and as a microkernel will likely stay that way. Much of the kernels
functions, such as memory management, syscalls etc. are implemented in the runtime. This leaves only the most basic
functions in the kernel, such as interrupt handling, physical memory management and the like.

### WASM Runtime

The WASM runtime is the heart of k23, it is responsible for running WebAssembly programs. It is not a standalone crate,
but implemented as part of the kernel since it is so core to the system. The runtime uses the `wasmparser`
and `cranelift` crates to parse and compile the WASM programs.

Currently, the runtime is quite simple, it only supports the most basic WASM instructions and features.

TODO this section will expand with more info.