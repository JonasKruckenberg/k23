# k23 build configuration

The target architecture is determined by which `k23_image` target you build:

```sh
buck2 build //sys:k23-riscv64           # RISC-V 64-bit
buck2 build //sys:k23-aarch64           # AArch64
buck2 build //sys:                      # all architectures
```

Override configuration values on the command line as needed:

```sh
buck2 build //sys:k23-riscv64 -c k23.log_level=trace
```

## Running under QEMU

```sh
buck2 run //sys:k23-riscv64-qemu
buck2 run //sys:k23-riscv64-qemu -c k23.log_level=trace
```
