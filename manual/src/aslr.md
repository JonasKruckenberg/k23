# Address Space Layout Randomization

Address-space layout randomization is a security technique that - as the name implies - randomizes
the placement of various objects in virtual memory. This well known technique defends against attacks such as 
return-oriented programming (ROP) where an attacker chains together instruction sequences of legitimate programs 
(called "gadgets") to archive privilege escalation.

Randomizing the placement of objects makes these techniques much harder since now an attacker has to correctly guess 
the address from a potentially huge number of possibilities.

## KASLR

k23 randomizes the location of the kernel, stacks, TLS regions and heap at boot time.

## ASLR in k23

k23 implements more advanced userspace ASLR that other operating systems, it not only randomizes the placement of 
WASM executable code, tables, globals, and memories; but also the location of individual WASM functions at each program 
startup (a similar technique is used by the Linux kernel called function-grained kernel address space layout randomization (FGKASLR))

TODO explain more in detail

## ASLR Entropy

Entropy determines how "spread out" allocations are in the address space
higher values mean a more sparse address space, this is configured through the `entropy_bits` option (TODO).

Ideally the number would be as high as possible, since more entropy means harder to defeat
ASLR. However, a sparser address space requires more memory for page tables and a higher
value for entropy means allocating virtual memory takes longer (more misses the search function
that searches for free gaps). The maximum entropy value also depends on the target architecture
and chosen memory mode:

| Architecture           | Virtual Address Usable Bits | Max Entropy Bits |
|------------------------|-----------------------------|------------------|
| Riscv32 Sv32           | 32                          | 19               |
| Riscv64 Sv39           | 39                          | 26               |
| Riscv64 Sv48           | 48                          | 35               |
| Riscv64 Sv57           | 57                          | 44               |
| x86_64                 | 48                          | 35               |
| aarch64 3 TLB lvls     | 39                          | 26               |
| aarch64 4 TLB lvls     | 48                          | 35               |

In conclusion, the best value for `entropy_bits` depends on a lot of factors and should be tuned
for best results trading off sparseness and runtime complexity for better security.

Note also that for e.g. Riscv64 Sv57 it might not even be desirable to use all 44 bits of
available entropy since the address space itself is already huge and performance might degrade
too much.