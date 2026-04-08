# Architecture Deep Dive

This document covers the internals of k23 in detail. For the high-level picture — bootloader, kernel, WASM runtime — start with the [Architecture Overview](../overview.md) and [System Startup](../startup.md).

## Workspace Layout

The repository is a Cargo workspace (edition 2024, resolver v3):

```
k23/
├── kernel/          # The OS kernel — no_std, primary target riscv64
├── loader/          # Bootloader (self-extracting, embeds the kernel binary)
├── loader/api       # Shared interface between loader and kernel (BootInfo, LoaderConfig)
├── libs/            # ~20 k-prefixed library crates, all no_std
├── crates/          # General-purpose standalone crates (no k-prefix)
├── build/           # Build tooling (xtask, toml-patch)
├── fuzz/            # Fuzzing harnesses (excluded from workspace)
├── tests/           # WAST spec test files
├── manual/          # This mdBook
└── configurations/  # Cargo target specs and build configurations
```

The key structural rule: code that must be shared between the loader and kernel lives in `loader/api` or a `libs/` crate. Code used only by the kernel stays in `kernel/`. Everything in `libs/` is `no_std`.

---

## Memory Management

Memory management is layered. Each layer builds on the one below.

### 1. Bootstrap Allocator

Before the kernel heap exists, a bump allocator (provided by the loader via `BootInfo`) handles early allocations. This is only used during the startup sequence, before `kmain` transitions to the main allocator.

### 2. Kernel Heap

The kernel heap is backed by [Talc](https://github.com/SFBdragon/talc) configured with `ErrOnOom` — allocating beyond available memory returns an error rather than panicking. The heap is initialized during the Main startup phase by carving out a region from the physical memory pool.

### 3. Frame Allocator (Physical Memory)

Physical memory is managed through a frame allocator in `libs/mem-core`. It maintains a pool of free physical page frames, organized into arenas. The loader populates the initial free list via the `MemoryRegion` entries in `BootInfo`. After loader memory is reclaimed (see [System Startup](../startup.md)), those physical frames are added to the pool.

The allocator is global (wrapped in a `Mutex`) and exposed through the `FrameAllocator` trait so the virtual memory subsystem can request and release physical pages without knowing the allocator's internals.

### 4. Virtual Memory

Virtual memory is managed through Virtual Memory Objects (VMOs). A VMO represents a contiguous region of virtual address space and tracks which physical frames (if any) back it. There are two kinds:

- **Eagerly mapped**: physical frames are allocated and mapped at VMO creation time.
- **Demand-paged**: frames are faulted in on first access, handled by the trap handler.

The kernel's own virtual address space layout is described in [Virtual Memory Layout on RISC-V](../arch/riscv/memory_layout.md).

### WASM Linear Memory and Guard Pages

WASM linear memory is a special case. Each WASM instance gets a `Memory` VMO with a 2 GiB *offset guard* region placed after the addressable memory. This means that a WASM load or store instruction that goes slightly out of bounds hits an unmapped page and traps — no explicit bounds check needed in the hot path. The guard size (2 GiB) is chosen so that even the largest possible WASM 32-bit offset (`0xFFFFFFFF`) cannot reach past the guard into live kernel memory.

---

## Async Executor

The async executor lives in `libs/async` (`kasync`). It implements cooperative multitasking — tasks yield voluntarily; there is no preemption at the task level (hardware interrupts still fire, but they don't context-switch tasks).

### Per-CPU Queues and the Injector

Each CPU has its own scheduler with a local LIFO run queue. A global injector queue allows tasks to be spawned from any context and picked up by any CPU. The per-CPU scheduler checks the local queue first (fast path, no synchronization needed), then falls back to the injector.

### Work Stealing

When a CPU's local queue is empty and the injector has nothing, the scheduler attempts to steal tasks from a randomly chosen peer CPU. The target is selected with a simple PRNG so the stealing pattern doesn't cluster on a single victim. Stealing is bounded — the scheduler won't spin indefinitely looking for work.

### Task Representation

A `Task` is a heap-allocated `Pin<Box<dyn Future<Output = ()>>>` plus metadata (task ID, waker). Tasks are stored as intrusive linked-list nodes so the scheduler can manipulate the run queue without additional allocation.

### Yielding

Long-running computations should call `kasync::yield_now()` periodically. This suspends the current task and re-queues it, giving other tasks a chance to run. The executor calls this implicitly at `.await` points that resolve immediately, but tight loops without natural suspension points need explicit yields.

---

## WASM Virtual Machine

The WASM VM is implemented inside the kernel (`kernel/src/wasm/`) rather than as a separate crate, because it is deeply integrated with kernel subsystems (memory management, trap handling, syscalls).

### Components

```
Engine  (global, Arc-wrapped)
  └─ TypeRegistry       — deduplicated WASM function types
  └─ Compiler           — Cranelift-based code generator
  └─ ASIDAllocator      — address space IDs for guest instances
  └─ EpochCounter       — interruption mechanism

Store  (per-task context)
  └─ Module[]           — parsed WASM binaries
  └─ Instance[]         — live instantiated modules
       └─ Memory        — linear memory VMO + guard pages
       └─ Table[]       — function/extern-ref tables
       └─ Global[]      — mutable/immutable WASM globals
       └─ Function[]    — compiled function handles
```

**Engine** is created once and shared across all WASM activity on all CPUs. It holds global-scope state that doesn't change per-instantiation: the type registry, the compiler, and the epoch counter used to interrupt runaway guests.

**Store** is the runtime context for a set of modules and instances. Each asynchronous task that executes WASM code has its own `Store`.

**Module** is the result of parsing a WASM binary with `wasmparser`. It holds the type section, function definitions, import/export tables, and the raw code section (bytecode to be compiled on demand or eagerly).

**Instance** is a live `Module` with all imports resolved and memories/tables allocated. Creating an instance runs the WASM `start` function if one is present.

### Compilation Pipeline

```
WASM binary
  → wasmparser      (decode + validate)
  → IR translation  (WASM opcodes → Cranelift IR)
  → Cranelift       (IR → native machine code)
  → mmap'd region   (executable, mapped into the instance's address space)
```

The compiler targets the host architecture (riscv64 in production, native arch for hosted tests). The optimization level is `speed_and_size` — a balance between compile time and runtime performance appropriate for a kernel that compiles modules at load time.

Function types are canonicalized through the `TypeRegistry` before compilation. Two WASM modules that import the same function signature get the same canonical type ID, which allows sharing compiled trampolines and simplifies indirect call dispatch.

### Epoch-Based Interruption

The `EpochCounter` provides a low-overhead way to interrupt executing WASM code. The compiler inserts epoch checks at loop back-edges and function entries. When the kernel increments the global epoch (e.g., on a timer interrupt), any WASM code that hits a check point and finds the epoch advanced will trap back to the kernel. This is how the scheduler can eventually reclaim CPUs from WASM guests without cooperative yields.

### Host Functions

WASM modules can call back into the kernel through *host functions* — Rust functions registered in the `Store` and exported to WASM under a module/function name. Host functions implement the WASM system interface (syscall layer). They run with full kernel privileges and can access kernel state, allocate memory, spawn tasks, etc.

---

## Trap Handling

Trap handling is architecture-specific. On RISC-V, the exception vector is installed during `arch::per_cpu_init_late`. All traps (synchronous exceptions and asynchronous interrupts) go through a single entry point in the `trap` crate.

The handler distinguishes:

- **WASM guest traps** (out-of-bounds memory, unreachable, divide-by-zero, etc.): unwound back to the Rust `Store` caller, which converts them to a `Trap` error value.
- **Kernel traps** (page faults, unaligned access, illegal instruction): demand-paging faults are handled by the VMO subsystem; all others cause a kernel panic with a backtrace.
- **Timer interrupts**: used to advance the epoch counter (to interrupt WASM) and to drive the async executor's timer facility.
- **External interrupts**: dispatched to the registered IRQ driver for the current CPU.

---

## Tracing

k23 uses `tracing` / `tracing-core` for structured logging and diagnostics. The subscriber is implemented in `kernel/src/tracing/` and is CPU-local — each CPU writes to its own output buffer to avoid cross-CPU locking in the hot path.

The subscriber stack:
1. **Filter**: parses the `RUST_LOG`-style filter string from boot arguments and gates which events pass through.
2. **Registry**: a lock-free sharded slab that stores active span data.
3. **Writer**: formats events and writes them to the configured output (semihosting UART during early boot, 16550 UART once the driver is up).

The `log` crate is bridged through a compatibility shim so that dependencies using `log::info!()` etc. produce `tracing` events.

---

## Testing Infrastructure

### Hosted Tests

Standard `cargo test` / `cargo nextest` tests. Used for code with no hardware dependencies: data structure implementations (`wavltree`, `range-tree`), parsers, pure algorithms. These run on the developer's machine.

### On-Target Tests (`#[ktest]`)

The `ktest` crate provides a custom test framework for code that needs the kernel environment. Tests are annotated with `#[ktest]` (from `libs/test/macros`). The macro generates a `Test` struct instance in a dedicated ELF section; the test runner discovers these at startup and executes them inside the async executor.

Each test binary is a separate ELF file linked with the kernel but with a test-runner entry point instead of `kmain`. The loader (which is generic over payloads) loads and boots each test binary independently in its own QEMU instance.

Tests return a `Pin<Box<dyn Future<Output = Outcome>>>`. `Outcome` is `Passed`, `Failed(Box<dyn Any>)`, or `Ignored`. The runner collects results and prints a summary with pass/fail/ignore counts.

```
just test-riscv64   # builds + runs all ktest binaries on QEMU
```

### Concurrency Tests (Loom)

Code that uses synchronization primitives (from `kspin`, `kasync`) can be tested under [Loom](https://github.com/tokio-rs/loom) by enabling `cfg(loom)`. Loom exhaustively explores thread interleavings, so concurrency bugs that are timing-dependent in normal tests become deterministic failures.

```
just loom
```

### WASM Specification Tests

`.wast` files in `tests/` are WASM specification test suites. The `wast!` macro (from `libs/wast`) compiles and runs each directive and asserts the expected outcome (trap, value, etc.). These verify WASM spec compliance as the runtime evolves.

---

## Adding a New Architecture

Architecture-specific code lives in `kernel/src/arch/<name>/` and `libs/riscv/` (for RISC-V specifics). To bring up a new architecture:

1. Add a new directory under `kernel/src/arch/` and implement the required functions:
   - `per_cpu_init_early` — FPU reset, performance counters, early CPU config
   - `per_cpu_init_late` — exception vector, interrupts, timer, IRQ driver
2. Add a Cargo target spec under `configurations/` for the new target triple.
3. Implement the `HardwareAddressSpace` trait from `libs/mem-core` for the architecture's MMU.
4. Wire up the trap entry point in `libs/trap` for the new ISA.
5. Add a `just test-<arch>` recipe that boots a QEMU instance for the new target.

AArch64 and x86_64 stubs exist in the codebase as placeholders — they do not compile to a functional kernel yet.
