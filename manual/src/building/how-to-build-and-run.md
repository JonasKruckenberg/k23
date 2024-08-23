# How to Build and Run K23

## Prerequisites:

The following tools are required to build and run k23:

- [Rust](https://www.rust-lang.org/tools/install) - k23 is written entirely in Rust
- [just](https://just.systems) - Just is the simple command runner that k23 uses
- [QEMU](https://www.qemu.org) - QEMU used to run the kernel in a virtual machine
- [Nix](https://nixos.org) OPTIONAL - Nix is used to manage the development environment

## Running

Type `just` to see the available actions to run. The one you are probably looking for is `just run-riscv64` which will
build k23 for `riscv64` and run it inside QEMU. Note that this is currently just running a few basic tests and exits.
Other actions include:

- `just preflight` which will run all lints and checks
- `just test configs/riscv64-qemu.toml` which will run all tests for Risc-V in QEMU
