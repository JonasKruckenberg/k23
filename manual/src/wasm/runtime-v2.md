# Runtime v2 — Fact-Check and Design Notes

Status: **design brainstorm**, 2026-07. This document fact-checks the rearchitecture pitch
(component-model-only, tiering JIT, stackless tasks, pooling allocation, extracted crate)
against the current codebase and against published data from other engines and
runtime-as-OS projects, then sketches an aggressively simple design. Sources are linked
inline; codebase claims reference the tree at the time of writing.

## 1. Where we start

An audit of the current tree reframes the project: this is not a refactor, it is a
green-field rewrite with an unusually free hand.

- `sys/kernel/src/wasm/` is **~24.5k lines** (~68% of the kernel crate) of a
  wasmtime-architecture port — Engine/Store/Linker/`VMContext`/array-call ABI —
  compiled eagerly with Cranelift. It is **dead code in production**: nothing outside
  the in-kernel test harness calls it, the whole module is `#![expect(dead_code)]`,
  and 81 `todo!()` sites remain (including `memory.grow`, `memory.copy`,
  `memory.fill`, all atomics builtins).
- Spec conformance is currently **zero**: the upstream testsuite submodule is empty and
  every spectest entry is commented out. Only 5 local `.wast` smoke tests run.
- There is **no pooling allocator** (`PlaceholderAllocatorDontUse` allocates instances
  from the kernel heap and never frees memories/tables), **no fiber machinery**, **no
  kasync integration**, and **no component-model support** (the vendored `lib/wast`
  fork can *parse* components; the runtime cannot).
- The wasm↔virtmem contract is four calls: `Mmap::new_zeroed`, `Mmap::make_executable`,
  copy-in, and `with_kernel_aspace`. The generic virtmem machinery behind it is
  ~4.5k lines (plus `wavltree` at 2.7k whose only consumer it is), and two orphaned
  crates (`lib/_kmem` 2.1k, `lib/range-tree` 4.9k) have no build-graph consumers at all.

Consequence: nothing needs to be kept green during the rewrite, and the honest baseline
for "does the rewrite carry its weight" is *zero working lines*, not 24.5k.

## 2. Fact-check of the pitch

### 2.1 Component model only, targeting 0.3 async — **correct, and cheaper than pitched**

WASI 0.3 was ratified June 2026 and rebases WASI onto the component model's native
async: `async func`, `stream<T>`, `future<T>`, completion-based (io_uring-like), with
`wasi:io` deleted and — critically — **the host owning a single shared event loop**
across all components. That is exactly a kernel's shape: the k23 executor *is* the
event loop.

One correction to the pitch's framing: *"lower v0.3 components into their stackless
continuation-passing lowering"* is not something the runtime needs to do. The
canonical ABI's **callback lift option already is that lowering**, and the *guest
toolchain* performs it (Rust `async fn` via wit-bindgen compiles to a state machine
that returns wait/yield/done codes and receives events through an exported callback).
The runtime's job is merely to (a) require the callback option and (b) run the event
loop. Doing the CPS transform in the runtime would be re-implementing Asyncify, whose
published costs (~1.3x runtime, ~50% code-size growth) are the reason the callback ABI
exists.

Fibers are only unavoidable for two things, both refusable at load time:

1. **Stackful lifts** — gated post-0.3 (🚟) and not in the shipped tier anyway.
2. **Sync-lowered calls that block** — i.e. WASI-0.2-style all-sync components calling
   async host interfaces. Wasmtime parks a pooled fiber for these
   (`WaitMode::Fiber` vs `WaitMode::Callback` in its concurrent runtime; callback
   tasks pin *no* fiber while suspended).

A kernel that only ever admits p3-native components can therefore ship **zero fiber
machinery**. corosensei is a good escape hatch if that constraint is ever relaxed —
it is `no_std`, supports riscv64, and switches in ~2–4ns — but it should be an
add-on behind the task abstraction, not a foundation.

### 2.2 Tiering JIT — **directionally correct; the biggest win is lazy, not tiered**

Published numbers, briefly: single-pass baseline compilers (Liftoff, SpiderMonkey
baseline, Winch, wasmer-singlepass) compile **15–20x faster** than optimizing tiers
(~50ns per wasm byte single-threaded) and produce code **1.1–2x slower**. Tier-up in
every production engine happens **per function, at call boundaries**, driven by simple
counters (V8: ~1.8M "bytes executed" budget; JSC: 150 calls to BBQ, 50k to OMG). V8
does no wasm OSR at all; wasm needs **no deoptimization** unless you speculate, and
speculation only pays for GC-heavy languages (V8's deopt machinery bought +1% on
SQLite-wasm). Copy-and-patch compiles another ~5x faster than Liftoff with *better*
code, but requires an LLVM stencil-generation build pipeline per target ISA.

Two refinements for the k23 case:

- **"The vast majority will never run" is an argument for laziness first, tiering
  second.** A function that is never called costs zero under lazy compilation
  regardless of which compiler would have compiled it. The startup win of a baseline
  tier over lazy-Cranelift is the *first-call latency* of functions that do run
  (µs vs ms per function). Real, but second-order compared to not compiling 90% of
  the graph at all. Validation is also separable: k23's store design (content-hashed
  `add` gate) gives a natural place to validate **once at add time**, so runtime
  compilation never re-validates.
- **"Tiering is required for lowering optimized component calls" — right instinct,
  slightly different mechanism.** Cross-component call optimization in wasmtime is
  fused adapters (lower∘lift compiled into one trampoline) plus **cross-module
  inlining in the optimizing tier** (3.69x on cross-component call-heavy benchmarks;
  landed Wasmtime 36, motivated explicitly by components). So the optimizing tier is
  where "fixed edges fuse into plain calls" happens — but a baseline tier is not a
  prerequisite for it. Also note the cautionary tale: after component-async landed,
  wasmtime's purely-sync cross-component calls regressed ~3.5x due to per-boundary
  task bookkeeping through a host intrinsic. Design the task state so the sync/fused
  path can keep it on the stack.

**riscv64 reality check:** Winch has no riscv64 backend, so a baseline tier means
writing a single-pass compiler ourselves (the design is well documented: abstract
operand stack over a register/stack-slot/constant lattice, lazy spill, canonical
layout at merges — Liftoff, SpiderMonkey baseline, and Titzer's CGO'24 taxonomy are
the references). That is the largest genuinely new artifact in this plan. Sequence it:
build the tiering *seam* (per-function dispatch slots + counters + backedge checks)
from day one, ship with lazy-Cranelift as the only compiler, add the baseline
compiler when boot-latency measurements justify it.

**Code publication on RISC-V** is the OS-flavored gotcha: `fence.i` only synchronizes
the executing hart, so patching a call *instruction* requires an IPI+`fence.i`
broadcast. Making all cross-function dispatch **data-indirect** (calls load a
per-function slot pointer; tier-up is an atomic store with Release) downgrades
patching to ordinary memory ordering — and the executor's scheduler loop provides the
`fence.i` point for newly published code (per-hart generation counter, fence when
stale). This costs one load per cross-function call in baseline code; the optimizing
tier devirtualizes fused/fixed edges anyway.

### 2.3 Stackless tasks + quiescent-state reclamation — **correct, and load-bearing**

The research record strongly supports building the kernel's deferred-reclamation
primitive on cooperative quiescence:

- QSBR has the cheapest possible read side (plain loads, zero barriers) *if* threads
  are guaranteed to announce quiescence — which a cooperative executor gets for free:
  **returning `Poll::Pending` is the quiescent state**. A suspended stackless task
  holds no stack, so it cannot hold a return address into JIT code — "wait one
  executor epoch on all harts" replaces the JVM's stop-the-world stack scanning for
  code unloading. Linux's RCU-tasks (built so ftrace trampolines can be freed) is the
  precedent that manufactures this property artificially; we own the scheduler, so we
  get it natively.
- The failure mode is the guest that doesn't yield (redshirt died on this; Go had to
  add signal-based preemption in 1.14). The fix is compiled-in **epoch checks on loop
  backedges** (wasmtime's epoch interruption; JVM safepoint polls cost ~0.5–1 cycle).
  These must be in the baseline tier from day one — they bound time-to-quiescence
  *and* are the preemption mechanism. One mechanism, three consumers: scheduling
  fairness, code reclamation, subgraph unlink.
- This also confirms the pitch's migration story: a paused callback task is a heap
  value (its linear-memory state + task-table entries), not a machine stack.

Scale check on "1M concurrent tasks": a callback task is table entries plus guest
state — no native stack — so 1M *tasks* is a memory-budget question, not a VA or
stack question. Note the canonical ABI's instance-wide lock serializes stackless
tasks *within* one instance (cooperative interleaving at yields, not parallelism);
throughput parallelism comes from many instances. That matches the pitch's grain:
components are the isolation unit, tasks are the concurrency unit.

### 2.4 Pooling allocation only — **correct, with one big fork in the road**

Everything wasmtime's pooling allocator does with memfd/madvise/MPK is userspace
emulation of powers this kernel has natively: direct PTE edits with no `mmap_lock`,
precise `sfence.vma` scoping, frame-granular CoW without files, the entire Sv48/Sv57
VA budget. Published wasmtime numbers (instantiation ~5µs with slot reuse + CoW +
lazy table init, teardown = one `madvise`) are the bar, and a kernel can beat it.

The fork: **guard pages or explicit bounds checks?**

- Guard-page slots cost ~6GiB VA each (4GiB + guard). Fine for thousands of memories;
  nowhere near 1M (and MPK striping is x86-only — no riscv64 equivalent). They also
  drag in the entire fault-attribution machinery: wasm trap handler, PC→code registry,
  non-local register restore from the trap frame — today ~750 lines of the subtlest
  code in the tree, plus arch glue.
- Explicit bounds checks cost 12.7% (VMIL'24 hybrid) to ~20%+ (naive) runtime on
  memory-heavy code, and in exchange: **zero VA reservation per memory, zero
  MMU/page-table state per instance, no signals/traps in the memory path at all**,
  identical semantics for memory64, and — decisively for this project — **the hosted
  test backend becomes trivial and bit-identical to the kernel backend** (no SIGSEGV
  handling; wasmtime itself ships this as `signals_based_traps=off` for its no_std
  embeddings).

Recommendation: explicit bounds checks, everywhere, from day one. It deletes the most
code, it is the only option that scales past the VA wall, and the performance gap is
recoverable in the optimizing tier (bounds-check hoisting/dedup, and the pitch's
fixed-edge fusion deletes boundary crossings entirely). The MMU remains for W^X code
publishing, the direct map, and *optional hardening* (isolate placement) — not for
sandbox correctness. This is also the honest response to the Spectre record (see §6):
the MMU was never going to be the isolation story here anyway.

With bounds checks chosen, "pooling" becomes almost embarrassingly simple: fixed slot
arrays for instance state/vmctx/tables (sized by kernel config), linear memories as
VA reservations sized to declared max with frames committed on grow, teardown = zap
PTEs + targeted `sfence.vma` + return frames. No wavltree, no region trees, no demand
paging, no per-guest address spaces, no ASID allocation. The generic virtmem machinery
(~4.5–7.2k lines) reduces to a static-layout allocator plus the code-publish path.
CoW instantiation images and lazy data-segment init are later optimizations the slot
layout should permit but v1 should not implement.

### 2.5 Extraction + hosted backend — **correct; bounds checks make it much stronger**

The runtime becomes a `//sys` crate with a small platform trait (reserve/commit/
protect-RX/icache-sync/time/spawn). Kernel backend implements it over the frame
allocator and kasync; hosted backend over mmap and std — *with no signal handling
needed* because all traps are explicit control flow. That enables, on the host:
upstream spec-testsuite runs, wasmtime's component-async conformance tests,
differential fuzzing against wasmtime (wasm-smith), proptest on canonical-ABI
lift/lower round-trips, loom on the task/waitable tables, and criterion benchmarks
(compile MB/s, call-boundary ns, instantiation µs, task churn) — none of which can
run under QEMU today. The in-kernel `.wast` selftests remain as the integration
layer.

## 3. The design, condensed

**One sentence: the kernel is the component-model event loop; everything else is a
compiler.**

1. **Admission**: components enter through the store's `add` gate. Validate once,
   there. Reject at load: stackful lifts, non-utf8 string encodings, and any core
   feature outside the supported set (§4). Content hash = identity (per the system
   pitch).
2. **Execution**: callback-ABI tasks scheduled directly as kasync tasks. CM task
   table, waitable-sets, backpressure counters are plain kernel data structures.
   `waitable-set.wait` lowers to the task returning Pending; delivery is the executor
   polling it with an event. No fibers, no guest-visible threads.
3. **Compilation**: per-function lazy, dispatch through data-indirect slots.
   Tier 0 = Cranelift (later: single-pass baseline with the same contract).
   Tier-up per function by counter, publish = atomic slot store, reclaim old code
   after one executor epoch (QSBR). Backedge epoch checks in all generated code.
   Optimizing tier owns fused adapters and cross-component inlining ("fixed edges
   fuse into plain calls").
4. **Memory**: explicit bounds checks; slot-pooled instances/tables/memories; no
   signals anywhere in the wasm path; traps are explicit branches to a libcall that
   unwinds synchronously to the activation entry. A trap kills the instance
   (Midori-style abandonment); the parent that linked it decides restart policy.
5. **Host boundary**: no generic linker, no `IntoFunc` machinery, no runtime type
   registry for host functions. The kernel presents the `k23:*` family as a
   **component implemented natively**: WIT → build-time codegen → static export
   tables, type-checked at build. There is exactly one call path in the system
   (component→component); the kernel is just a component whose exports are hardware
   handles. This answers the "do we need a wasmtime-style host interface" question:
   no — and it is a deletion, not a constraint.

## 4. Feature set

Core wasm: exactly LLVM's `lime1` contract (what default clang/rustc output needs) —
mutable-globals, sign-ext, multivalue, bulk-memory, nontrapping-fptoint,
extended-const, call-indirect-overlong — plus funcref tables.

Component model: callback-ABI async lift/lower, `stream`/`future`, waitable-sets,
backpressure, `context.get/set`, resources/handles, utf8 only.

Explicitly cut (all precedented by embedded runtimes and/or gated proposals): SIMD,
threads/atomics, GC proposal, exception handling (guests build with `panic=abort`),
memory64, multi-memory, tail calls, stackful lifts, non-utf8 transcoding, code
serialization, and every tunable that wasmtime exposes as `Config` — k23 has exactly
one configuration.

Non-negotiable debt to pay in the rewrite: wire the upstream spec testsuite for every
feature we claim. The current runtime claims features it does not test.

## 5. What gets deleted vs written

Deleted or superseded: `sys/kernel/src/wasm/` (24.5k), most of `sys/kernel/src/mem/`
+ arch mem glue (~4.5k, retaining a static-layout allocator + code publish),
`lib/wavltree` as a dependency of the above (2.7k), orphans `lib/_kmem` (2.1k) and
`lib/range-tree` (4.9k) regardless.

Written: the new crate — plausibly 12–18k lines all-in for v1 (translate layer can be
salvaged from the current tree; `lib/wast`'s component parsing already exists for the
test harness; Cranelift stays a third-party dep). Net line count goes *down* while
gaining components, async, pooling, tests, fuzzing, and benchmarks — because the
generic-embedder surface area (Store<T> generics, typed-func machinery, dynamic
linker, config space, signal handling) is where wasmtime spends its complexity, and
a kernel needs none of it.

## 6. Risks

- **Spec flux.** Stackful lifts, threads, stream splicing, caller-supplied buffers,
  and the CM-1.0 "lazy value handle" ABI redesign are all in motion. Mitigation:
  isolate lift/lower behind one module; everything cut in §4 is also everything in
  flux.
- **Spectre.** Both ring-0 wasm predecessors (Nebulet, kwast) stalled with
  transient-execution attacks as the acknowledged open problem; the 2024+ industry
  answer is hardware isolation underneath (Mewz, Hyperlight) or compiler hardening
  (Swivel). The pitch's "no fusing across trust boundaries" rule is the right hook;
  bounds-check masking is the known mitigation on the memory path. v1 should state
  this as a documented non-goal with the hooks in place, not pretend SFI alone is a
  security boundary against a determined co-resident attacker.
- **Guest toolchain maturity.** All-callback-only means p3-native guests only; the
  practical question is how good `wit-bindgen`'s async Rust output is today. Needs a
  spike before committing the fiber-free constraint.
- **Baseline compiler scope.** A riscv64 single-pass compiler is multi-month work.
  The lazy-first sequencing de-risks it; the dispatch-slot contract must be designed
  so the baseline drops in without touching the runtime.
- **Cross-component sync-call overhead.** Wasmtime's 3.5x async-bookkeeping
  regression on sync calls is the cautionary tale; task-state layout must let fused
  sync paths keep task state on the native stack.

## 7. Selected sources

- WASI 0.3 launch, Bytecode Alliance (2026): <https://bytecodealliance.org/articles/WASI-0.3>
- Component-model Concurrency explainer: <https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md>
- Wasmtime component-async runtime (`concurrent.rs`), fused adapters (FACT), inliner: <https://bytecodealliance.org/articles/inliner>, <https://bytecodealliance.org/articles/the-road-to-component-model-1-0>
- Liftoff design: <https://v8.dev/blog/liftoff>; V8 wasm pipeline (lazy compile, jump-table patching, no OSR): <https://v8.dev/docs/wasm-compilation-pipeline>
- Titzer, *Whose Baseline Compiler Is It Anyway?* (CGO 2024): <https://arxiv.org/abs/2305.13241>
- Xu & Kjolstad, *Copy-and-Patch Compilation* (OOPSLA 2021): <https://arxiv.org/abs/2011.13127>
- Winch baseline RFC: <https://github.com/bytecodealliance/rfcs/blob/main/accepted/wasmtime-baseline-compilation.md>
- Wasmtime 1.0 performance (CoW, lazy init, 5µs instantiation): <https://bytecodealliance.org/articles/wasmtime-10-performance>
- Pooling allocator + MPK striping: <https://docs.wasmtime.dev/api/wasmtime/struct.PoolingAllocationConfig.html>, <https://docs.wasmtime.dev/examples-mpk.html>
- Bounds-check costs: *Performant Bounds Checking for 64-Bit WebAssembly* (VMIL 2024): <https://dl.acm.org/doi/10.1145/3689490.3690400>; *Leaps and Bounds* (IISWC 2022): <https://tom-spink.com/papers/iiswc22leaps.pdf>
- LLVM `lime1` CPU definition: <https://github.com/llvm/llvm-project/pull/112049>
- RISC-V CMODX / `fence.i` scope: <https://docs.kernel.org/arch/riscv/cmodx.html>
- RCU-tasks (quiescence for trampoline reclamation): <https://lwn.net/Articles/607117/>
- Singularity isolation cost measurements: *Deconstructing Process Isolation* (2006): <https://dl.acm.org/doi/10.1145/1178597.1178599>
- Midori retrospectives (error model, async-everything): <https://joeduffyblog.com/>
- Nebulet retrospective: <https://lsneff.me/why-nebulet>
- Swivel (Spectre hardening for SFI wasm), USENIX Security 2021: <https://www.usenix.org/conference/usenixsecurity21/presentation/narayan>
- corosensei: <https://github.com/Amanieu/corosensei>
