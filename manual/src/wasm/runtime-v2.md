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

Recommendation (revised after review): **guard-page slots by default, explicit checks
as a per-memory mode — both, because memory64 forces the second to exist anyway.**
A 12–20% flat tax on every memory access in every component, drivers included, is not
acceptable when the whole system is components, and unlike boundary costs it cannot
be fused away. The VA math for guard slots works out once the kernel owns the whole
space: at ~6 GiB per slot (4 GiB reservation + 2 GiB guard; rare static offsets
beyond the guard get explicit checks, as in wasmtime), an Sv48 kernel half
(128 TiB) holds ~20k concurrent memories and Sv57 ~11M — and tasks outnumber
instances by orders of magnitude, so "1M tasks" never meant 1M linear memories.
Sv39 (256 GiB half, ~40 full-size slots) is the constrained case: there the planner
packs small-max memories into proportionally small slots or falls back to checked
mode. Checked mode must exist regardless because 64-bit memories cannot elide checks
via guards (Titzer's >1 TiB secondary guard region and similar schemes are later
optimizations), and it is also the deterministic, signal-free mode the hosted
backend wants for fuzzing. Cranelift already parameterizes heap lowering per memory,
so dual-mode is configuration, not a second compiler.

The honest cost of keeping guards: the wasm fault path stays — trap-handler
integration, PC→code lookup, non-local register restore — simplified but not
deleted, and hosted guard-mode parity needs SIGSEGV handling (or guard-path coverage
stays with the in-kernel selftests while host-side fuzzing runs checked mode).
Guards are not a Spectre answer either way — speculative OOB needs masking
regardless (Swivel) — so the hardening story is orthogonal to this choice.

Either way, the pool itself stays simple: fixed slot
arrays for instance state/vmctx/tables (sized by kernel config), linear memories as
VA reservations sized to declared max with frames committed on grow, teardown = zap
PTEs + targeted `sfence.vma` + return frames. No wavltree, no region trees, no demand
paging, no per-guest address spaces, no ASID allocation. The generic virtmem machinery
(~4.5–7.2k lines) reduces to a static-layout allocator plus the code-publish path.
CoW instantiation images and lazy data-segment init are later optimizations the slot
layout should permit but v1 should not implement.

### 2.5 Extraction + hosted backend — **correct; bounds checks make it much stronger**

The runtime becomes a `//sys` crate with a small platform trait (reserve/commit/
protect-RX/icache-sync/trap-hook/time/spawn). Kernel backend implements it over the
frame allocator and kasync; hosted backend over mmap and std, running checked-memory
mode (§2.4) for deterministic, signal-free fuzzing, with optional SIGSEGV handling
for guard-mode parity. That enables, on the host:
upstream spec-testsuite runs, wasmtime's component-async conformance tests,
differential fuzzing against wasmtime (wasm-smith), proptest on canonical-ABI
lift/lower round-trips, loom on the task/waitable tables, and criterion benchmarks
(compile MB/s, call-boundary ns, instantiation µs, task churn) — none of which can
run under QEMU today. The in-kernel `.wast` selftests remain as the integration
layer.

### 2.6 Speculation at component boundaries — guard-and-fallback yes, deopt no

Review question: should inter-component calls be optimized speculatively (same-hart
vs cross-hart, co-resident vs remote), and does that pull deoptimization back in?
The useful distinction is between **guards at call boundaries** (cheap, no metadata,
fallback is an ordinary call) and **speculation whose invalidation must interrupt a
live frame** (requires deopt/OSR machinery: side tables mapping machine state back
to abstract frames). Everything the component model invites falls in the first
bucket:

1. **Eager completion.** The async ABI's status codes explicitly allow a call to
   report "returned" without ever suspending — so the fast path is: run the callee
   synchronously on the caller's hart and only materialize a subtask/enqueue if it
   actually suspends or hits backpressure (lazy task creation, Cilk-style). This is
   the single biggest boundary optimization, and it is spec-blessed rather than
   speculative in the deopt sense.
2. **Handle devirtualization.** Calls on resource handles are interface-typed but
   implementation-varying — the polymorphic call sites of this system. Inline
   caches / feedback-guided guarded inlining ("this handle is almost always the
   uart-driver instance") work as in SpiderMonkey's wasm call inlining: a failed
   guard falls back to the indirect call *within* optimized code. No frame
   invalidation, no deopt metadata.
3. **Placement.** Same-hart vs cross-hart execution is a scheduler decision that the
   CM's confined nondeterminism already permits; it needs no compiler support.
4. **Edge lifetime.** Fixed/stable edges are static fusion, not speculation.
   Removable edges stay behind patchable dispatch slots; re-linking takes effect at
   call boundaries after an executor epoch — and since unlink must quiesce anyway
   (handle teardown), a stale inlined target cannot outlive its subgraph.

Deopt/OSR would only become necessary for speculation that must be undone *mid-loop,
before the next call boundary* — e.g. inlining a removable edge into a hot loop.
Nothing in the model requires that; keeping removable-edge calls out-of-line is the
one discipline that keeps deopt out of the system permanently.

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
4. **Memory**: slot-pooled instances/tables/memories; guard-page elision for wasm32
   by default, per-memory checked mode for memory64/Sv39/dense packing (§2.4).
   Non-memory traps (unreachable, div, casts) are explicit branches to a libcall
   that unwinds synchronously to the activation entry. A trap kills the instance
   (Midori-style abandonment); the parent that linked it decides restart policy.
5. **Host boundary**: no string-keyed runtime linker, no `IntoFunc` machinery, no
   runtime type registration for host functions. The kernel presents the `k23:*`
   family as a **component implemented natively**: WIT → build-time codegen → static
   export tables, type-checked at build. There is exactly one call path in the
   system (component→component); the kernel is just a component whose exports are
   hardware handles. A linker *of some sort* remains, but it is **graph
   instantiation**: walk the resolved edge table in dependency order and fill each
   instance's import vector with concrete funcrefs/adapters. Names exist only in
   WIT tooling at build time; at runtime, `linker.start` (per the system pitch)
   takes handles, never names. One slim engine-level type interner stays —
   `call_indirect` needs canonical func types on day one, and GC casts will need
   the same table later.
6. **Device I/O is WIT all the way down** (decided in review): MMIO, DMA, and
   interrupts are ordinary WIT interfaces (`k23:mmio`, `k23:dma`, `k23:irq`)
   whose resources *are* the capabilities — an MMIO-region handle is the right
   to touch those registers, a DMA-buffer handle is the right to that memory
   (IOMMU-fenced), an irq handle yields a `stream` of events (top half stays
   native; the wake is the delivery). Two implementations of one contract:
   the **reference implementation is plain host calls** — which is what the
   hosted backend uses against device models, making drivers testable and
   fuzzable off-target — and the optimizing tier may recognize these interfaces
   and lower calls on fixed edges to inline volatile accesses with the region
   base patched in at instantiation, reaching native-driver code quality.
   Three disciplines make this sound: (a) MMIO must *never* be mapped into a
   wasm linear memory — wasm has no volatile semantics and any optimizer may
   merge or elide accesses (invariant 1); interface calls are the only correct
   surface. (b) The interface contract must state ordering explicitly
   (read/write plus an explicit fence operation), because once calls inline,
   the compiler — not a host function — is responsible for emitting the fences
   (invariants 1–2). (c) Granularity: resources carry control and ownership;
   bulk data crunching happens in linear memory after one bulk transfer or via
   handle pass-through — a per-byte interface call in a packet loop is a design
   bug regardless of how well it lowers.

## 4. Feature roadmap

**Day-1 floor.** Core wasm: LLVM's `lime1` contract (what default clang/rustc output
needs) — mutable-globals, sign-ext, multivalue, bulk-memory, nontrapping-fptoint,
extended-const, call-indirect-overlong — plus funcref tables. Component model:
callback-ABI async lift/lower, `stream`/`future`, waitable-sets, backpressure,
`context.get/set`, resources/handles, utf8.

**Adopted proposals are a commitment, not a corner to cut.** Wasm 3.0 (Sept 2025)
standardized GC, memory64, tail calls, multi-memory, and exception handling, and
toolchains will progressively assume them. None are architecturally excluded here,
but each leaves a hook to respect now:

- *Tail calls, SIMD*: compiler-tier features — another reason to lead with
  lazy-Cranelift (which has both) and let the hand-written baseline catch up later.
- *memory64*: requires the checked-memory mode (§2.4) — which is why that mode is
  kept even though guards are the default.
- *Exception handling*: needs unwind/trap metadata in the `Compiler` contract from
  the start, even while guests build with `panic=abort`.
- *GC*: needs the type interner (kept anyway for `call_indirect`) and precise roots —
  where stackless helps enormously: at quiescent points **no wasm frames exist**, so
  the root set is heap task state + tables + globals, and "GC at yield points only"
  falls out of the execution model instead of requiring stack maps for every pc.
- *Multi-memory*: falls out of per-memory heap configuration.

**Deferred while in flux**: shared-everything threads (the one item that could force
a real redesign — it breaks the instance-lock cooperative model; CM's cooperative
threads come first in the spec pipeline anyway), stackful lifts, non-utf8
transcoding, stream splicing, code serialization.

**Cut on principle**: wasmtime's `Config` space. k23 has exactly one configuration.

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

## 6. Cross-cutting optimizations: what owning the whole stack buys

Added after review. The question: kernel, scheduler, virtmem, QSBR, runtime, and DMA
allocator are one codebase — what can be optimized *out* of the system that layered
stacks must keep, and what do we have over other wasm runtimes specifically?

### 6.1 The pattern in the published data

Every multi-x systems win of the last decade is the same move — deleting a boundary
crossing that a general-purpose stack must keep: IX's run-to-completion dataplane
(memcached 3.6–6.4x vs Linux, RTT halved), Arrakis's kernel-off-the-data-path
(Redis 2–9x), XDP vs the full network stack (24 vs 4.8 Mpps/core), ScyllaDB's
shard-per-core vs threads+locks (3–10x, vendor numbers), Unikraft's whole-image LTO
(nginx/Redis 1.7–2.7x vs Linux guests), Singularity's software isolation (<5%
overhead vs 25–33% for hardware domains). k23's design already deletes the two
biggest crossings — the syscall boundary and the protection-domain switch. The
second-generation dataplane papers (Shenango, Caladan, Shinjuku) carry the equally
important inverse lesson: after deleting the layers you must rebuild scheduling and
interference control at ~5 µs granularity, which is only possible when one system
owns cores, queues, and interrupts together. That is kasync's job description, not
an accident.

### 6.2 Versus other kernels

1. **Interrupts are wakeups; tickless by construction.** Top half = wake the
   driver's task; bottom half is the task. Linux's threaded IRQs and Intel UINTR
   (notification in 0.73 µs vs 15.3 µs via signals) measure how much generic
   delivery layers cost; `nohz_full` shows Linux fighting to get *down to* one
   residual tick per second. A cooperative kernel has no tick to remove: the timer
   wheel programs the hardware comparator to the next actual deadline. Top halves
   stay native kernel code — running wasm in IRQ context is a category error
   (invariants 4/8). *Verdict: real, nearly free, day one.*
2. **Home-hart instances, not task work-stealing.** The canonical ABI's
   instance-wide lock serializes execution within an instance, so **the parallel
   unit of this system is the instance no matter what the scheduler does** —
   stealing individual tasks can never extract parallelism from one instance, but
   it forces every cross-component call and every counter (tier-up, IC feedback,
   backpressure) to be synchronized "just in case." Give every instance a home
   hart instead: its tasks always run there, the instance lock becomes implicit
   (hart-serialism), counters are plain increments, eager-completion calls to
   co-resident instances need zero synchronization, and teardown's `sfence.vma`
   targets one hart. Burstiness — the legitimate worry, and why pure Seastar-style
   static sharding fails general workloads (ZygOS measured 1.26x over
   run-to-completion IX from fixing exactly this) — is handled by making the
   *instance* the migration unit: a load-triggered rebalancer (Shenango-style
   queue-delay signal) moves whole instances between harts, which stackless makes
   trivially safe — at quiescence an instance is heap state and an atomic
   home-pointer, no stack to move. Home assignment is adaptive at runtime
   (planner labels are initial hints only), so nothing needs to be known at build
   time. Migration respects placement labels (never onto a hart shared with a
   distrusting side-channel-sensitive instance). Kernel-internal tasks (compile
   jobs, housekeeping) stay in an ordinary stealable pool — two task classes, not
   one. The pressure valve for a single instance saturating its hart is component
   sharding (N instances of a stateless component — the planner can autoscale),
   which is the same answer ScyllaDB gives for hot shards; no scheduler can
   parallelize what the ABI serializes. *Verdict: real, and the instance-lock
   argument makes it strictly better than task-stealing for guest work; the cost
   is a rebalancer policy that doesn't exist yet (worst case degrades to
   Seastar-static, which still works).*
3. **Run-to-completion + eager completion + placement.** The planner co-locates
   chatty components on one hart so the eager-completion fast path (§2.6) makes a
   cross-component async call an ordinary call in the common case; IX's adaptive
   batching applies at the waitable-set level under load. *Verdict: real; mostly
   falls out of §2.6 plus edge labels.*
4. **DMA-stable guest memory, zero-copy I/O.** Linear memories never move in the
   slot design, so guest memory is DMA-safe *by construction*: the IOMMU maps a
   slot once at instantiation; there is no `get_user_pages`, no pinning API, no
   registered-buffer machinery — io_uring's registered buffers (worth 3–11% in
   published measurements) exist to emulate this from userspace. `k23:*` device
   interfaces should pass DMA buffers as *resources* (handles), so payload copies
   happen only when a guest actually reads the bytes into its memory — or never,
   for forwarding topologies. *Verdict: real and differentiating. Boundary: the
   CM's shared-nothing model makes one copy the floor for `stream<u8>` payloads
   between components — accept it until the CM's caller-supplied-buffer/lazy-handle
   work lands; do not invent bespoke shared memory (see 6.4).*
5. **Hugepages deterministically.** Hot JIT code packed into 2 MiB text pages
   (Meta measured −50% iTLB misses, −5–10% CPU fleet-wide; Intel's large-code-pages
   blueprint −30% iTLB misses), 1 GiB direct map, 2 MiB-backed large guest
   memories (TCMalloc's hugepage-aware allocator: +7.7% RPS). Linux gets these
   probabilistically via THP/khugepaged; we get them by construction — the frame
   allocator and the code region are the same subsystem. *Verdict: real, cheap,
   bake into the allocator/code-region layout now.*
6. **TLB economics.** Single address space → zero context-switch flushes, global
   mappings; teardown PTE zaps batched at epoch boundaries → `sfence.vma` per
   epoch, not per unmap, scoped to the harts that ran the instance. *Verdict:
   real; falls out of QSBR + SAS.*
7. **One grace period for everything.** QSBR serves RCU data structures, JIT code
   unload, slot recycling, and subgraph unlink — one mechanism where Linux carries
   RCU + RCU-tasks + deferred work queues, and V8 carries a code-GC with stack
   scanning. *Verdict: real, but see 6.4: epochs must not become the system clock.*
8. **Handle tables are kernel tables.** A canonical-ABI handle index dereferences
   to a slot the kernel owns directly — no fd-translation layer, no refcount churn
   (lifetime = ownership transfer + QSBR). Singularity's exchange-heap numbers
   (803 cycles ping-pong, but O(1) for any payload size) bound this from below;
   with checks compiled into JIT'd code, a sandbox crossing approaches a plain
   call. *Verdict: this is just the §3 design, stated as an optimization.*
9. **vDSO for free.** Clock/entropy imports lower to loads from a host-updated
   page and inline through fixed-edge fusion — the vDSO trick with no ABI to
   maintain. *Verdict: tiny, real, do it.*
10. **Mitigation by label.** Spectre fences/masking/core-exclusion applied only on
    trust-crossing edges, because only we know where those are; conventional
    systems mitigate globally. *Verdict: real in principle, unproven in practice;
    design the label plumbing, defer the mitigations themselves.*
11. **Prebaked instantiation.** The graph is known at build: vmctx layouts, import
    vectors, and data-segment images can be precomputed so boot instantiation is
    memcpy-plus-patch (wasmtime's 5 µs per instance is the userspace ceiling;
    build-time preparation goes below it). Tension with ASLR aspirations —
    relocation at image-bake or loader time, decided deliberately. *Verdict: real
    but later; measure instantiation first.*
12. **A JIT-aware scheduler.** No mainstream OS scheduler consumes JIT-queue
    information (the JVM's compiler threads fighting container CPU limits is the
    canonical pathology). Tier-up jobs as idle-priority kasync tasks with
    backpressure is novel, nearly free here, and structurally impossible for a
    userspace runtime that can't see system idleness. *Verdict: real novelty,
    cheap.*

### 6.3 Versus other wasm runtimes and JIT compilers

1. **Closed-world-per-graph compilation.** Wasmtime and V8 must assume an open
   world — any module may link at any time, against any embedder. Our component
   graph is explicit data: devirtualization and inlining driven by edge *labels*
   rather than heuristics, and export tree-shaking falls out of lazy compilation
   (an export no edge reaches is simply never compiled). How closed is the world,
   exactly? With generation (NixOS-style) semantics it stratifies precisely along
   the edge-lifetime labels — `store.add` admits *bytes*, not linkability;
   linkability is reachable-from-pins, and the pin set is fixed by the root hash:
   - **Fixed and stable edges are truly closed within a boot** (stable changes
     only at generation switch): bake assumptions in freely — fuse, inline,
     delete the call. Stable edges need undo records only if generation switches
     ever become live rather than reboot-shaped.
   - **Holes** are dynamic in *when* they fill, but their candidate sets are
     known at build (the pins) and only ever shrink (`narrow` is monotonic). So
     hole-filling can be speculated on safely — adapters for every candidate can
     even be compiled ahead of time — and shrinkage is a revocation event that
     already implies quiescence (unlink). Growth, the case that breaks
     closed-world compilers, **cannot be expressed**.
   - **Removable edges** are the only genuinely open surface, and they carry the
     patch-point discipline (§2.6).

   One flag: this makes the developer edit-compile-run loop a generation-switch
   loop. The escape valve already exists in the model — a dev profile labels the
   app's edges removable, making iteration a live re-link of a removable subgraph
   instead of an image rebuild — but it must be treated as a first-class
   requirement, not an afterthought, or the model punishes its own developers.
2. **No embedder-generality tax.** No `Store<T>` generics, no runtime `Config`, no
   fuel (epochs only), no `.cwasm` serialization-stability contract, no
   SEH/DWARF/frame-pointer interop for foreign profilers and debuggers, no
   public-API semver. This is where a large share of wasmtime's complexity
   (and our deleted 24.5k lines) actually lives.
3. **Scheduler-native async.** Wasmtime bridges three worlds — CM event loop,
   tokio, and fibers — paying Send/Sync/Pin impedance, pooled-fiber hops, and a
   host-intrinsic call per boundary crossing (their measured ~3.5x sync-call
   regression is this tax made visible). Here the CM task *is* the kasync task;
   with shard-per-core there are no Send bounds and task state for fused sync
   paths lives on the native stack.
4. **`-march=native`, always.** We compile on the machine we run on, every time —
   full ISA-extension use (vector when SIMD lands) with no baseline-CPU shipping
   compromise and no multi-variant artifacts. Amend the pitch's "recompile after
   boot" with a content-addressed cache keyed `(component hash, cpu, tier)` —
   treated as cache, not source of truth — so weak hardware doesn't recompile the
   world every boot.
5. **Snapshot by construction.** A quiescent instance is a heap value: no stacks,
   no fibers, no register state. Checkpoint, clone (CoW), migrate, and
   pre-initialize (live Wizer) become runtime primitives rather than external
   tools — wasmtime cannot snapshot mid-async at all (suspended fibers are
   machine stacks). This is the pitch's migration story earned structurally.
6. **Profile infrastructure for free.** Baseline counters + inline-cache feedback
   give the optimizing tier real PGO on every run (wasmtime AOT compiles blind);
   owning the timer additionally allows sampling-based hotness with zero
   instrumentation — but counters are needed for call-site feedback anyway, so:
   counters first, sampling as a later augment, not a replacement.
7. **Code layout and code GC owned end-to-end.** Call-graph-ordered packing of hot
   functions *across components* into shared hugepage text (BOLT/AsmDB-class wins,
   ~5–10% CPU, deterministic here); dead-tier reclamation is one QSBR epoch versus
   V8's refcounting-plus-stack-scan code GC. Compilation runs on idle harts with
   global knowledge (6.2.12) instead of competing blindly with the workload.
8. **Validate and translate once, ever.** Content-addressed store → validation and
   translation artifacts persist per hash across boots and instances; other
   engines re-validate per process or carry serialization-compat machinery.
9. **No cohabitation taxes.** No JS VM (V8/JSC pay handle scopes and boundary
   conversions), no POSIX emulation, no signal-safety contortions in trap
   dispatch — the trap frame is simply in hand.

### 6.4 Pushback ledger

Ideas considered and rejected, or bounded — kept here so they don't return by
osmosis:

- **"1M concurrent tasks" as a design driver.** Stackless callback tasks make 1M
  structurally cheap (heap state only), and that's where its influence should end.
  Name the workload before any *further* contortion is justified by this number.
- **Bespoke shared memory between components** to beat the one-copy stream floor:
  breaks the shared-nothing model that the whole handle/isolation story rests on.
  Wait for CM caller-supplied buffers / lazy value handles; pass host-owned DMA
  buffers as resources meanwhile.
- **Wasm interrupt top-halves.** Entry cost isn't the issue; epoch semantics,
  backpressure, and the no-alloc/no-panic trap context are. Native top half, task
  bottom half, permanently.
- **Page coloring for LLC isolation.** Dead on modern hashed/sliced LLCs (~2
  usable color bits); RISC-V's Ssqosid/CBQRI has no public silicon numbers yet.
  Rely on the placement labels (don't share cores across sensitive pairs); revisit
  when CBQRI hardware exists.
- **Per-guest address spaces for defense-in-depth.** Re-pays the 25–33% hardware
  isolation tax the whole design exists to avoid. The MMU hardens code (W^X) and
  backs slots; it is not the sandbox.
- **Epochs as the universal clock.** QSBR reclaims; it must not *pace*. Nothing
  latency-sensitive may wait on a grace period (allocation paths stay
  epoch-independent), and backedge checks bound laggard harts.
- **Over-coupling compiler and scheduler.** The full contract between them stays
  exactly two items: backedge epoch checks and placement/edge labels. A compiler
  that knows more about the scheduler (or vice versa) welds shut the seams that
  make the rest of this document's deletions possible.

## 7. Risks

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

## 8. Selected sources

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
- IX dataplane OS (OSDI 2014): <https://www.usenix.org/system/files/conference/osdi14/osdi14-paper-belay.pdf>
- Arrakis (OSDI 2014): <https://www.usenix.org/system/files/conference/osdi14/osdi14-paper-peter_simon.pdf>
- Shenango (NSDI 2019): <https://www.usenix.org/conference/nsdi19/presentation/ousterhout>; Caladan (OSDI 2020): <https://www.usenix.org/conference/osdi20/presentation/fried>; Shinjuku (NSDI 2019): <https://www.usenix.org/conference/nsdi19/presentation/kaffes>
- Demikernel (SOSP 2021): <https://dl.acm.org/doi/10.1145/3477132.3483569>
- Seastar shard-per-core / `foreign_ptr`: <https://docs.seastar.io/master/split/24.html>; thread-per-core tail-latency study (ANCS 2019): <https://penberg.org/papers/tpc-ancs19.pdf>
- XDP (CoNEXT 2018): <http://borkmann.ch/paper/2018_xdp.pdf>; io_uring: <https://kernel.dk/io_uring.pdf>
- BOLT (CGO 2019): <https://arxiv.org/abs/1807.06735v1>; Meta hot-text + hugepages: <https://link.springer.com/chapter/10.1007/978-3-030-23499-7_10>; Intel large code pages: <https://www.intel.com/content/dam/develop/external/us/en/documents/runtimeperformanceoptimizationblueprint-largecodepages-q1update.pdf>; AsmDB (ISCA 2019): <https://liberty.cs.princeton.edu/Publications/isca19_frontend.pdf>
- TCMalloc Temeraire, hugepage-aware allocation (OSDI 2021): <https://www.usenix.org/system/files/osdi21-hunter.pdf>
- Linux `nohz_full`: <https://kernel.org/doc/html/v5.8/timers/no_hz.html>; OS-noise measurement: <https://www.iris.sssup.it/bitstream/11382/548111/1/IEEE-TC-2022.pdf>
- Intel UINTR latency numbers (LibPreemptible): <https://arxiv.org/pdf/2308.02896>; RISC-V AIA/IMSIC v1.0: <https://docs.riscv.org/reference/aia/v1.0/MSLevel.html>
- Unikraft (EuroSys 2021): <https://dl.acm.org/doi/10.1145/3447786.3456248>
- ERIM, MPK domain switching (USENIX Security 2019): <https://people.mpi-sws.org/~druschel/publications/erim.pdf>
- LLC slice-hash reverse engineering (why page coloring is dead on modern Intel): <https://eprint.iacr.org/2015/690.pdf>; RISC-V CBQRI: <https://lwn.net/Articles/929553/>
