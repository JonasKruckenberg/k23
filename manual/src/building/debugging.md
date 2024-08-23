# Debugging k23

The rest of this guide assumes you are using LLDB, but the same principles apply to GDB and "command translation guides"
are available online.

### Attaching to the Kernel

You can run the kernel with the `--debug` or `--dbg` (or `--gdb` for typos) flag to start the kernel in a paused state.
You can then launch and attach to the kernel with LLDB using the following commands:

```sh
rust-lldb target/riscv64gc-unknown-k23-kernel/debug/kernel

# In LLDB
gdb-remote localhost:1234
```

### Catching Panics

Quite often, you will need to stop the kernel when a panic occurs, to inspect the state of the system. For this you can
set a breakpoint on the `rust_panic` symbol which is a special unmangled function for exactly this purpose (this
technique mirrors Rusts `std` library and is implemented in the `kstd`
crate [here](https://github.com/JonasKruckenberg/k23/blob/07322361bd99c04d8a6866fd8a5c565584393222/libs/kstd/src/panicking.rs#L89)).

Using LLDB you can set a breakpoint with the following command:

```
b rust_panic
```

and then use e.g. the `bt` command to print a backtrace.

### Pretty Printing

To make debugging easier, you can add pretty printers for the `vmm::PhysicalAddress` and `vmm::VirtualAddress` types.
This can be done by through the following commands in LLDB:

```
type summary add --summary-string "vmm::PhysicalAddress(${var.0%x})" vmm::PhysicalAddress
type summary add --summary-string "vmm::VirtualAddress(${var.0%x})" vmm::VirtualAddress
```