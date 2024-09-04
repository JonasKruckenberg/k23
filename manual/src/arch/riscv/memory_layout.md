# Virtual Memory Layout on RISC-V

This page outlines the virtual memory layout used by *k23* depending on the selected memory mode.
Currently supported memory modes are `Riscv64Sv39`, `Riscv64Sv48` and `Riscv64Sv57`.
Note that addresses marked as `<dynamic>` are not fixed and depend on the number of harts (hardware threads) in the
system.

The code implementing this memory layout can be found in [`loader/src/mapping.rs`](../../loader/src/mapping.rs).

### Sv39

| Address Range                           | Size        | Description                                         |
|-----------------------------------------|-------------|-----------------------------------------------------|
| 0x0000000000000000..=0x0000003fffffffff | 256 GB      | user-space virtual memory                           |
| 0x0000004000000000..=0xffffffbfffffffff | ~16K PB     | hole of non-canonical virtual memory addresses      |
|                                         |             | kernel-space virtual memory                         |
| 0xffffffc000000000..=\<dynamic\>        | ~96 GB      | unused                                              |
| \<dynamic\>..=\<dynamic\>               | \<dynamic\> | kernel stacks                                       |
| \<dynamic\>..=0xffffffd7ffffffff        | \<dynamic\> | kernel TLS (thread local storage)                   |
| 0xffffffd800000000..=0xffffffe080000000 | 124 GB      | direct mapping of all physical memory (PHYS_OFFSET) |
| 0xffffffff80000000..=0xffffffffffffffff | 2 GB        | kernel (KERN_OFFSET)                                |

### Sv48

| Address Range                           | Size        | Description                                         |
|-----------------------------------------|-------------|-----------------------------------------------------|
| 0x0000000000000000..=0x00007fffffffffff | 128 TB      | user-space virtual memory                           |
| 0x0000800000000000..=0xffff7fffffffffff | ~16K PB     | hole of non-canonical virtual memory addresses      |
|                                         |             | kernel-space virtual memory                         |
| 0xffff800000000000..=0xffffbfff7ffefffe | ~64 TB      | unused                                              |
| \<dynamic\>..=\<dynamic\>               | \<dynamic\> | kernel stacks                                       |
| \<dynamic\>..=0xffffbfff7fffffff        | \<dynamic\> | kernel TLS (thread local storage)                   |
| 0xffffbfff80000000..=0xffffffff7fffffff | 64 TB       | direct mapping of all physical memory (PHYS_OFFSET) |
| 0xffffffff80000000..=0xffffffffffffffff | 2 GB        | kernel (KERN_OFFSET)                                |

### Sv57

| Address Range                           | Size        | Description                                         |
|-----------------------------------------|-------------|-----------------------------------------------------|
| 0x0000000000000000..=0x00ffffffffffffff | 64 PB       | user-space virtual memory                           |
| 0x0100000000000000..=0xfeffffffffffffff | ~16K PB     | hole of non-canonical virtual memory addresses      |
|                                         |             | kernel-space virtual memory                         |
| 0xff00000000000000..=0xff7fffff7ffefffe | ~32 PB      | unused                                              |
| \<dynamic\>..=\<dynamic\>               | \<dynamic\> | kernel stacks                                       |
| \<dynamic\>..=0xff7fffff7fffffff        | \<dynamic\> | kernel TLS (thread local storage)                   |
| 0xff7fffff80000000..=0xffffffff7fffffff | 32 PB       | direct mapping of all physical memory (PHYS_OFFSET) |
| 0xffffffff80000000..=0xffffffffffffffff | 2 GB        | kernel (KERN_OFFSET)                                |
