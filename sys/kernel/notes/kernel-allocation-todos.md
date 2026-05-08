# Kernel allocation reliability & performance TODOs

Companion to `alloc-callsites.md`. Items are ordered by reliability impact
first (eliminating allocations entirely or making them fail-safe), then
performance/fragmentation impact, then exploratory work.

Each item lists **change**, **why it matters**, **expected outcome**, and
**effort**.

---

## Tier 0 — eliminate allocations (reliability)

### T0.1 — Remove `Arc<HartNotify>` per-hart heap allocation

- **Change:** rewrite `sys/kernel/src/arch/riscv64/block_on.rs` to store
  `HartNotify` directly as a `#[thread_local] static` (or via `cpu_local!`
  with a `const`-init initializer). The waker vtable holds `*const HartNotify`
  and `clone`/`drop` become no-ops.
- **Why:** the static lives for the lifetime of the hart, so any waker the
  hart can produce is bounded by the static's lifetime. The current `Arc`
  refcount is structurally unused.
- **Outcome:** -1 alloc per hart on the very first `block_on` call. Removes
  a heap dependency from a path that may run before the global allocator is
  fully usable in some boot paths.
- **Effort:** S (~30 LoC; tricky lifetime around `Waker::from_raw`).

### T0.2 — Eagerly initialise every `Counter`'s per-hart slots at boot

- **Change:** keep `CpuLocal<AtomicU64>` per `counter!()` but iterate the
  `.bss.kcounter.*` link sections at boot (the macro already places each
  counter into a uniquely-named section) and force-allocate buckets for
  every counter for every known hart. This can be a single
  `metrics::init(num_harts)` call after `state::init_cpu_local`.
- **Why:** today every counter holds a `cpu_local::collection::CpuLocal<AtomicU64>`
  which lazily allocates power-of-two buckets per hart on first increment.
  Across hundreds of counters this is a cold-path-but-unbounded allocation
  fountain on metric paths, which is the wrong place for it.
- **Outcome:** all counter bucket allocations happen at one well-defined
  cold-path moment instead of being scattered across whatever code path
  hits a metric first. No `MAX_HARTS` requirement; the actual hart count
  comes from `boot_info.cpu_mask.count_ones()`.
- **Effort:** S — needs a linker-section iterator (similar to the existing
  `__start_k23_tests` / `__stop_k23_tests` pattern in `tests/mod.rs`).

### T0.3 — Eager-allocate the remaining `CpuLocal<T>` collections

- **Change:** at boot, replace
  - `tracing::registry::Registry::current_spans = CpuLocal::new()`
  - `mem::frame_alloc::FrameAllocator::cpu_local_cache = CpuLocal::new()`
  with `CpuLocal::with_capacity(boot_info.cpu_mask.count_ones())` so all
  buckets are allocated up front.
- **Why:** lazy bucket allocation can fire on first use of an irq path or
  a frame-allocator path; eager init moves these to a single cold boot
  step.
- **Outcome:** removes O(log N) allocs from runtime hot paths. No new BSS
  cost.
- **Effort:** XS, but requires plumbing the hart count through to both
  constructors.

### T0.4 — Stop reallocating the ASID bitmap per `WastContext`

- **Change:** `sys/kernel/src/arch/riscv64/asid_allocator.rs:52` allocates
  `vec![0; bitmap_size]` (8 KB in tests) on every `WastContext::new_default`.
  Either:
  1. Cache and reset one `WastContext` across selftest cases, or
  2. Allocate the bitmap once at engine init from a hart count derived at
     runtime, and reset by zeroing in place when the engine is reused.
- **Why:** the inventory marked this as static-after-init; it isn't, in the
  test build. 8 KB × N tests is the largest per-test repeating allocation.
- **Outcome:** -8 KB × test count. Brings the trace closer to real kernel
  behaviour.
- **Effort:** S.

### T0.5 — Pre-reserve a trap-path bump arena

- **Change:** `arch/riscv64/trap_handler.rs:391,423` does `Box::new(payload)`.
  Replace with allocations from a per-hart bump region. The region itself
  is allocated from the heap once during `arch::per_cpu_init_late` and
  stored in a `#[thread_local]` pointer; the trap path bumps from it and
  resets on return. No `MAX_HARTS` needed because the slab is owned by
  whichever hart actually came up.
- **Why:** trap allocations should never fail because the heap is full or
  fragmented. Today they go through `Talc` like anything else.
- **Outcome:** trap path becomes infallible on the alloc side. Removes one
  reason a kernel panic could turn into a double-fault.
- **Effort:** M (touches unwinding code; needs a lifetime story for trap
  payloads that survive the bump-reset).

---

## Tier 1 — pool/slab uniform-size hot allocations

### T1.1 — `FrameListNode` slab

- **Change:** add a slab allocator typed as `Slab<FrameListNode>`; route
  the four `Box::pin(FrameListNode { ... })` callsites in
  `mem/frame_alloc/frame_list.rs` through it.
- **Why:** every page-range insert into a wavltree allocates one fixed-size
  node. This is the canonical Linux `kmem_cache` use case.
- **Outcome:** O(1) alloc/free, no fragmentation, page-grained backing.
  Preserves intrusive-tree pinning.
- **Effort:** M (need a slab impl; can probably build on existing
  `lib/sharded-slab`).

### T1.2 — `AddressSpaceRegion` slab

- **Change:** same treatment for `mem/address_space.rs:412,476`
  (`Box::pin(AddressSpaceRegion)`).
- **Why:** uniform size, intrusive wavltree.
- **Outcome:** as above.
- **Effort:** S once T1.1 lands.

---

## Tier 2 — per-compile-job bump arena (the big one)

### T2.1 — Land a per-compile-job arena and route cranelift/regalloc2 through it

- **Change:** wrap one compile job with a `bumpalo::Bump`, expose it as a
  `core::alloc::Allocator`, and pass it down where regalloc2 / cranelift
  accept an allocator argument. For sites that don't accept an allocator
  parameter, switch the local Vecs to `Vec::new_in(&bump)`.
- **Why:** trace shows ~370 KB of transient allocation per compile, almost
  entirely `Vec::push` doubling chains and BTreeMap leaves. All have the
  same lifetime as the compile job. Bump arena collapses this to a few
  `Bump::new_chunk` calls and a single drop.
- **Outcome:** order-of-magnitude drop in allocation count for the WASM
  compile selftest. Removes 10 of the top-20 stacks. Dramatically reduces
  Talc fragmentation pressure during the heaviest workload.
- **Validation:** rerun `alloc_trace`; expect the regalloc2 / cranelift
  stacks (#1, #2, #3, #8, #10, #11, #12, #14, #18, #20) to disappear.
- **Effort:** L. The cranelift-codegen API surface is the work; bumpalo
  is already a kernel dep.

### T2.2 — `Vec::with_capacity` for kernel-owned compile sites

- **Change:** small targeted fixes:
  - `sys/kernel/src/wasm/compile/mod.rs:216` — collect with capacity from
    the iterator's `size_hint`.
  - `sys/kernel/src/wasm/cranelift/compiler.rs:518` — push into a Vec
    with known final size.
- **Why:** even before T2.1 lands, these remove visible doubling chains.
- **Outcome:** kills stack #5 (47 KB) and #18 (6 KB) from the trace.
- **Effort:** XS each.

---

## Tier 3 — per-CPU reusable scratch buffers

### T3.1 — Per-CPU scratch Vecs for the small transient call sites

- **Change:** introduce a `cpu_local!`-backed `RefCell<SmallVec<[T; N]>>`
  per scratch site, drained on each entry instead of dropped/re-allocated.
- **Sites:**
  - `wasm/types.rs:1482,1490` `wasm_params` / `wasm_results`
  - `wasm/cranelift/env.rs:1483,1750` `real_call_args`
  - `wasm/trap_handler.rs:507` `frames` (unwind buffer)
  - `mem/address_space.rs:729` `actions`
- **Why:** these are short-lived, called repeatedly, sized by the input and
  not the compile-job lifetime — so they don't fit T2.1's arena. A per-CPU
  reusable buffer that grows once and clears on entry covers them.
- **Outcome:** drops the small-Vec churn that T2.1 doesn't catch.
- **Effort:** S per site, S–M total.

---

## Tier 4 — investigations / open questions

### T4.1 — Symbolizer allocation strategy

- **Investigate:** the killswitched run hides ~70 % of real-world allocations.
  When the kernel actually prints a backtrace (panic, trap, log), the
  `addr2line`/`gimli` line-table parsing allocates heavily in transient
  Vecs. Two leads:
  1. Per-symbolize-call bump arena, scoped to one `find_frames` invocation.
  2. Build a static symbol table at link time so runtime symbolization is
     pointer arithmetic plus string slicing.
- **Outcome:** kernel panics that allocate heavily today become alloc-light
  or alloc-free, which matters because panic paths often run with a damaged
  heap.

### T4.2 — `kasync` task allocator policy

- **Investigate:** `sys/async/src/task.rs:937` does `Box::new(Task::new(...))`
  per spawn. The `Task` size is monomorphized per future type, so the size
  distribution depends on what's spawned. Take a histogram of task sizes
  over a real workload, then decide between (a) a small-medium-large size
  class slab dedicated to tasks, (b) a global slab keyed on the rounded-up
  size, or (c) leaving it on the heap. Also consider whether the executor
  should own a bump arena that can free all tasks at once on shutdown.

### T4.3 — `device_tree` bumpalo chunk sizing

- **Change:** trace shows three chunk allocations (4, 8, 16 KB) during
  device-tree parse. `bumpalo` doubles each new chunk.
- **Investigate:** profile typical device-tree size, then call
  `Bump::with_capacity(...)` once at the start of `device_tree::parse` so
  no chunk growth is needed.
- **Effort:** XS once we have the size.

### T4.4 — Confirm the runtime tracer matches a real workload

- **Investigate:** the current trace is dominated by the WASM compile
  selftest. (B) and (C) — `FrameListNode` and `AddressSpaceRegion` — are
  predicted-hot but invisible in this run. Re-run the tracer under a
  workload that exercises mmap / page faults so we have concrete numbers
  before committing T1.x slab work.

---

## Suggested execution order

1. T2.2 + T0.4 — XS, immediate trace improvements.
2. T0.1 + T0.2 + T0.3 — eliminate per-hart heap dependencies.
3. T0.5 — trap-path safety.
4. T2.1 — the architectural win; consume most of the remaining trace.
5. T1.1 + T1.2 — slab work, validated against a re-run trace under a
   mmap-heavy workload (T4.4).
6. T3.x and T4.x as follow-up.

After each tier, re-run `alloc_trace` and update `alloc-callsites.md` §I.
