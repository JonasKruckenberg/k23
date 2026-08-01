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
2. **Handle dispatch.** Correction from review: in the CM as specified, resource
   method calls are **not** polymorphic. Resource types are generative and an
   imported resource type binds to exactly one exporting instance at
   instantiation, so a method call is a direct call to a link-time-known
   function; the handle is *data* (an index into the owner's handle table
   yielding the rep), not a dispatch mechanism. With a generation-closed graph
   this makes essentially every cross-component call site monomorphic-per-
   generation — so no inline-cache machinery is needed for handles at all.
   Polymorphism reappears only where the system chooses it: mux/registry
   components (dispatch on rep *inside* guest code — ordinary core-wasm
   indirection), wrapper splices (link-time graph edits → recompile/patch, still
   monomorphic between edits), and guest-internal `call_indirect`/`call_ref`
   (the classic core-wasm case, where V8's data says feedback buys little for
   non-GC languages). Speculation machinery accordingly shrinks to: nothing, for
   v1.
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

### 2.7 Framing correction: isolation is tiered, and the MMU is used, not forgone

Earlier drafts called this a "single address space" with "software instead of
hardware isolation." That is backwards and worth correcting, because the caricature
would lead to actually leaving hardware on the table — which is not the intent. Two
axes were conflated:

- **What *defines* an isolation boundary** — the compiler/JIT (a bounds check it
  inserts) or the MMU (a page table that faults).
- **What *hardware accelerates* enforcement** — guard pages, ASIDs, the IOMMU,
  W^X bits, CFI extensions.

These are independent, and the design uses hardware aggressively on the second axis
regardless of the first. The default memory mode (§2.4) is **guard pages**, which
means the fine-grained sandbox check is *already hardware-accelerated*: the boundary
is compiler-defined (base+bound the JIT knows), but the enforcement is a hardware
page fault, not a compare-and-branch. "SFI" here is a hardware/software co-design,
not a software substitute for hardware. The right principle is exactly the reviewer's:
**use the hardware primitive wherever it is cheap at the required granularity.** The
design already does, at three tiers:

1. **Component ↔ component (highest frequency).** Boundary is compiler-defined
   (bounds checks / guard-page traps), because the crossing must cost a call, not a
   context switch. Making this an MMU boundary would reintroduce microkernel-IPC
   cost — here "more hardware isolation" is *slower*, so the axis is not "more
   hardware" but "right primitive for the crossing rate." Hardware still enforces
   (guard-page fault) and hardens (W^X on the JIT code).
2. **Isolate ↔ isolate (low frequency, already a trust/semantic boundary).** Boundary
   is MMU-defined: a distinct address space per isolate, ASID-tagged, switched by the
   scheduler at isolate hand-off (not on any call path). This is real hardware
   isolation exactly where crossings are rare enough to amortize it — and it is the
   one mechanism with a track record against transient-execution attacks (it is what
   Chrome site isolation and kernel PTI are). It also multiplies the guard-slot VA
   budget (each isolate's page table maps only its own slots). See §6 / the security
   notes for when to actually enable it.
3. **Device ↔ memory.** IOMMU fences every driver's DMA (§3.6); the MMU backs the
   frame-granular CoW images and the hugepage code/direct-map layout (§6.2.5).

Further hardware to fold in as it matures (surfaced by the security research):
RISC-V CFI (Zicfilp landing pads / Zicfiss shadow stack) to harden JIT code control
flow, speculation-barrier CSRs / the `fence.t` line of proposals, and on other
targets ARM MTE / PAC and ultimately CHERI. None of these are forgone; several are
simply not ratified-with-silicon on RISC-V yet.

So the accurate one-liner is not "software isolation instead of hardware." It is:
**tiered isolation — the boundary mechanism is chosen per crossing-frequency, and
hardware accelerates or enforces at every tier.** The wins catalogued in §6 come from
deleting *conservative generality* (unknown-counterparty APIs, per-call MMU switches
that buy nothing at that granularity), not from declining hardware.

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
7. **Isolates are exposed to init as authority, not syscalls.** The kernel starts
   exactly one component — `init` — and hands it a root `linker` plus the
   hardware-handle mint (per the pitch). An **isolate is just a linker whose
   started instances share one address space and one encryption key**; the key is
   minted inside the isolate and is not expressible in any interface, so it cannot
   leak by construction. Creating one is an authority init holds (`isolate.new` →
   returns that isolate's linker), gated upstream by a keeper that mints the
   isolate handle only once its lock's evidence is presented. Cross-isolate edges
   are ordinary handle grants that the build labeled trust-crossing (fusion off,
   barriers on). Programmer's view: init is a component like any other, but for
   most systems it is **generated from declarative config by `k23-build`** — the
   planner resolves placement and labels at build time (§3.8), init merely enacts
   the resolved plan and fills holes as evidence arrives (boot, login, hotplug).
   Hand-authoring init against the `k23:link`/`k23:isolate` WIT is the power-user
   path, not the common one. Nothing here is a syscall surface — it is capability
   plumbing in a component.
8. **Placement is relocation-based linking (§3.8).** Instantiating the graph is the
   *loading* half of dynamic linking with the *symbol-lookup* half deleted (the
   build already resolved every name to a concrete component pair). See §3.8 for
   the mechanism; the key point for the pipeline is that compiled code is emitted
   position-independent with **relocations preserved to placement time**, so one
   cached `(hash, cpu, tier)` compilation is placed — with fresh randomization —
   at every boot without recompiling.

### 3.8 Placement, relocation, and the one patchable-edge primitive

Placement — deciding where each instance's code, memory, tables, and the targets of
its edges physically land, then making the code reference those addresses — is k23's
equivalent of dynamic linking, and the reviewer's instinct to steal the linker's
tricks is right. Precisely: we keep the *relocation/loading* half and drop the
*symbol-resolution* half, because the closed-per-generation graph (§6.3) already
resolved names to concrete pairs at build. What remains is patching addresses, which
is exactly what enables randomization without recompilation.

**Two patch strategies, chosen by edge label — the same split as everywhere else:**

- **Baked relocations** for fixed/stable edges and for placement addresses (memory
  base, table/global bases, direct call targets). The address is an immediate/PC-
  relative operand the compiler left as a relocation record; placement fills it in.
  Result: a plain direct call or `base+offset` with no indirection at run time.
  Patching mutates *code*, but placement patches **before the code ever executes**,
  so the hard RISC-V cross-modifying-code problem (§2.2) degenerates to its easy
  case: patch all relocations, one `fence.i` per hart that will run it, then start —
  no concurrent execution, no IPI dance. Placement-time is the *only* time code
  modification is cheap, and fixed edges live here.
- **Data slots** for removable edges and tier-up targets: the call loads a target
  pointer from a per-edge word and calls through it. One extra load at run time, but
  patching is a plain store under ordinary Release ordering, patchable *while
  running*, reclaimed via QSBR — no icache flush, no `fence.i`, no IPI.

The unification worth naming: **placement (dynamic linking), tier-up (code-version
swap), and removable-edge re-link are one "patchable edge" primitive at two speeds.**
Fixed/rare → code relocation (fast call, patch once). Removable/live → data slot
(one load, patch anytime). The edge labels that already drive fusion, mitigation, and
inlining also pick the patch mechanism — no new concept, just a fourth reading of the
same labels.

**Randomization falls out for free.** Emitting per-function code with inter-function
calls as relocations *is* function-granular ASLR: place each function at an
independently randomized address and patch the calls — the FGKASLR-like design
`manual/src/aslr.md` aspires to, delivered without the per-boot recompilation that
document assumed, because relocation patching is memcpy-plus-a-few-stores against a
cached compilation, not a Cranelift run. Cranelift already emits relocations (the
current tree resolves them in `link_and_finish`); the only pipeline change is to
*preserve* them to placement rather than resolve all at compile time.

**"Ad-hoc compiled stubs" = copy-and-patch at the link layer.** For edges that want
a specialized trampoline (a fused adapter, a mitigation-wrapped call), pre-bake the
stub as a copy-and-patch *stencil* (§2.2's C&P reference) with holes for the concrete
addresses/offsets, and stamp it out at placement — machine-code-quality glue for the
cost of a memcpy and a few patches, no per-edge Cranelift invocation. This is how
placement stays sub-recompile even when edges need real code, not just an address.

**One caveat to state plainly:** baked code is position-*dependent* after patching —
it cannot move again without re-patching. That is fine because **code never
migrates**: instance *migration* (§6.2.2) moves heap state, and the target either
already has the component's code placed or re-derives it from the hash. Only data
moves; code is re-placed, never relocated live. Don't let the "relocations" framing
smuggle in an assumption that placed code is freely movable — it isn't, and nothing
needs it to be.

### 3.9 Memory object taxonomy and address-space layout

The complete set of object kinds the address-space allocator must place. First a
load-bearing simplification that shapes everything below: **guest code runs in S-mode
(the same privilege as the kernel), isolated by SFI, not by a user/kernel privilege
split.** That is the whole "host calls are plain calls" bet — a U→S trap is the
syscall cost we delete. Consequence for layout: there is no U-bit distinction to
manage (no `SUM`/`PTE.U` games), every mapping is a supervisor mapping, and per-isolate
separation is achieved purely by *which frames each isolate's page table maps* — an
isolate simply has no PTE for another isolate's memory. The MMU is used for guard
pages, W^X, and isolate separation, never for a privilege boundary.

Three placement domains: **global** (mapped identically, global-bit, in every
isolate's page table), **shared code** (RX, global, hugepage-backed), and
**per-isolate** (the only region that differs between address spaces).

| # | Object | Scope | Prot | Backing | Notes |
|---|--------|-------|------|---------|-------|
| 1 | Kernel image (text/rodata/data/bss) | global | RX/RO/RW | ELF frames | the kernel itself |
| 2 | Direct map of physical RAM | global | RW | all RAM | frame allocator + host courier (below); talc heap lives here (invariant 7) |
| 3 | Page-table frames | global | RW | frames | one root per isolate + shared kernel-half tables |
| 4 | Per-hart scheduler + trap stacks | global | RW | frames | `sscratch` trap stack; guard page each (corosensei later for recover/switch) |
| 5 | Per-hart CPU-local block | global | RW | frames | `cpu-local`; run queue head, current-task ptr, epoch counter |
| 6 | Engine metadata: type interner, code registry, handle tables, task/subtask/waitable tables, backpressure counters | global (kernel heap) | RW | talc | canonical-ABI handle tables *are* these (§6.2.8); not guest-addressable |
| 7 | JIT code (per component×cpu×tier) | shared | RX | frames, 2 MiB | one copy shared by all instances of a component across all isolates; content-hashed public bytes, so global RX is fine (§7); reached only by JIT call instrs, never addressable by guests |
| 8 | Code side-tables: trap-PC table, unwind info, dispatch slots, C&P stub region | shared | RO / RW(slots) / RX(stubs) | frames | dispatch slots are the data-slot patch points (§3.8), RW; stubs RX |
| 9 | Pristine linear-memory init images | global | RO | frames | one per component; mapped CoW into each instance's memory (§2.4) |
| 10 | `vmctx` (VMContext) | per-instance | RW | frames | base pointers, import vector, builtin table; reached via a register; kernel-written, in the isolate's region |
| 11 | Linear memory 0 (heap) | per-instance | RW | guard slot | the big object: reserve max+guard, commit on grow (§2.4) |
| 12 | Additional linear memories (multi-memory) | per-instance | RW | slot / DMA frames | e.g. a driver's DMA memory (below) |
| 13 | Tables (funcref) | per-instance | RW→RO | frames | lazy-init; often RO after start |
| 14 | Globals | per-instance | RW | inline in vmctx / frames | usually folded into vmctx |
| 15 | Task state (CM callback task = kasync task) | per-active-call (kernel heap) | RW | talc | the continuation heap value; `context.get/set` 2-slot array; **no native stack** — this is the snapshot-by-construction property |
| 16 | DMA regions (buffers + descriptor rings) | per-driver-isolate | RW | IOMMU-mapped frames | cacheable RAM, known phys addr, per-device IOMMU domain; exposed as `k23:dma` resource *or* as a second linear memory (below) |
| 17 | MMIO regions (device registers) | per-driver-isolate | RW, non-cacheable | device phys | `k23:mmio` resource; base patched into inlined volatile accessors (§3.6) |
| 18 | Interrupt-controller pages (IMSIC/APLIC) | global | RW, non-cacheable | device phys | kernel-only; per-hart interrupt files |

**How async inter-component calls are expressed — and why they need no buffer region.**
This is the key realization for layout: the CM async ABI does **not** introduce a
dedicated per-call or per-queue memory region. An async call's arguments and results
are lowered into the *participants' own linear memories* (the callback ABI passes them
by pointer); the "queue" is the executor's run queue holding task refs (object 15,
kernel heap); `stream`/`future` are *unbuffered* — `read`/`write` rendezvous and copy
directly between the reader's and writer's linear-memory buffers. So the only new
storage an async call needs is its **task state** (object 15) — a heap value — plus
handle-table entries (object 6). No shared "channel buffer" region exists in the base
design.

The one subtlety is **cross-isolate** copies: if the two components live in different
isolate address spaces, neither maps the other's memory, so the kernel is the
**courier** — it copies through the direct map (object 2) by physical address, touching
both frames without either isolate sharing a mapping. This is Singularity's
exchange-heap ownership-transfer without a dedicated exchange heap: the copy is direct
and physical-address-mediated, one memcpy, host-driven. (Intra-isolate, or on a fixed
fused edge, the copy is just a `memcpy` the compiler emits — §6.2.4's one-copy floor.)
If the 0.3.x *buffered* stream/splicing features are later adopted, their buffers would
be a new kernel-heap pool, not a guest region — flag it then.

**DMA specifically** (answering "where do DMA buffers live"): a DMA buffer is cacheable
RAM that is (a) frame-backed with a known physical address, (b) placed in a per-device
IOMMU domain, and (c) reachable by the driver. Two ways to expose (c), decided per
interface: as a `k23:dma` **resource** the driver reads through interface calls (one
copy, simplest, safe), or — for zero-copy hot paths — as a **second linear memory** of
the driver instance (object 12): the driver does ordinary loads/stores against memory
index 1, whose frames are the IOMMU-mapped DMA region. DMA buffers are normal cacheable
memory (unlike MMIO), so no volatile-per-access concern — the ordering that matters is
the *completion* boundary (device-done → CPU-read), handled by the completion `future`
resolving with a fence (§3.6), not by treating the buffer as volatile. Descriptor rings
are the same object kind. Because slots never move (§2.4), a DMA region is DMA-safe for
its whole lifetime with a single IOMMU map at instantiation — no pin/unpin churn.

**Address-space structure that falls out:** every isolate's page table is
`[ global kernel half (objects 1–9, 18, identical everywhere, global-bit) ]` +
`[ this isolate's guest region (objects 10–17 for its instances only) ]`. Switching
isolates (rare, at scheduler hand-off — §2.7) swaps only the guest region via
`satp`+ASID; the global half never changes, so kernel text/heap/code/direct-map stay
TLB-resident across the switch. Within an isolate there is no per-call or per-instance
address-space change at all.

**Open placement decisions** (not blockers, but flag now): (a) whether `vmctx` and
tables sit in the guest region or a kernel-only sibling region the JIT reaches by
register — leaning kernel-side-but-isolate-local, since guests reach them only through
trusted JIT code; (b) whether small instances share a memory slot's guard reservation
(dense packing) or always get a full slot — a §2.4 knob; (c) exact VA carve-up of the
guest region between memories/tables/vmctx and the randomization budget for each
(§3.8).

### 3.10 Sv48 layout and arenas

Sv48 gives a 48-bit VA sign-extended from bit 47 to 64 bits — two canonical 128 TiB
halves with a faulting hole between (identical shape to x86-64). Page granularities:
4 KiB, 2 MiB, 1 GiB. With per-isolate address spaces (§2.7), the **higher half is the
shared global kernel** (identical PTEs, global bit, in every isolate's page table) and
the **lower half is that isolate's private guest region**. This gives a free, load-
bearing invariant: **any guest-reachable address is lower-half; anything higher-half is
kernel/shared** — a one-bit provenance check aligned with invariant 5, and the reason
an isolate switch only reloads the lower half (§3.9).

```
Sv48 — bit-47 sign-extended.  Two 128 TiB canonical halves.

0xFFFF_FFFF_FFFF_FFFF ┌─────────────────────────────────────────────────┐ ▲
                      │  device / MMIO / IMSIC   (non-cacheable, phys)    │ │
                      │  per-hart stacks + cpu-local   (wired, guarded)   │ │ GLOBAL
                      │  pristine init images   (RO, component-backed)    │ │ KERNEL
                      │  JIT code   (RX, shared, 2 MiB hugepages)         │ │ HALF
                      │  direct map of all RAM   (RW, 1 GiB pages;        │ │ — mapped
                      │      talc kernel heap lives here, inv. 7)         │ │ identically
                      │  kernel image   (RX/RO/RW, KASLR slide)           │ │ into every
0xFFFF_8000_0000_0000 └─────────────────────────────────────────────────┘ ▼ isolate
                      ╳╳╳   non-canonical hole — always faults   ╳╳╳
0x0000_7FFF_FFFF_FFFF ┌─────────────────────────────────────────────────┐ ▲
                      │         …randomization gap…                       │ │ PER-ISOLATE
                      │  linear-memory slots  (~6 GiB: 4 GiB max+guard;   │ │ GUEST HALF
                      │      demand-commit, CoW, swappable, relocatable)  │ │ — this
                      │        …randomization gap…                        │ │ isolate's
                      │  table / vmctx slots                              │ │ instances
                      │        …randomization gap…                        │ │ only;
                      │  DMA regions  (wired, IOMMU-mapped) [driver iso]  │ │ reloaded on
0x0000_0000_0000_1000 │  null guard                                       │ │ isolate
0x0000_0000_0000_0000 └─────────────────────────────────────────────────┘ ▼ switch
```

Sub-region sizes are illustrative; the exact carve-up and per-region entropy budget are
the open decisions from §3.9. Sv57 (five levels, 128 PiB per half) is a drop-in if VA
ever gets tight — but per-isolate lower halves make that unlikely (each isolate gets a
fresh 128 TiB).

**Arenas.** Grouping the §3.9 objects by the memory *policy* they need, not by what
they are:

| Arena | Objects | Prot | Initial content | Reclaim under pressure | Movable | Shared |
|---|---|---|---|---|---|---|
| **Wired-core** | kernel img, direct map, page tables, talc heap, per-hart stacks, cpu-local, dispatch slots, vmctx, globals, engine/handle/waitable tables | RW / RX | frames, ELF | **none — pinned** (inv. 7/8) | no | global |
| **Code** | JIT code, trap/unwind side-tables | RW→**RX** (W^X) | compiler output | **discard + recompile** (recomputable; no write-back) | no (position-dependent, §3.8) | yes (1 copy / component×cpu×tier) |
| **Image** | pristine linear-memory init images | RO | **component data segments** (from store) | **discard + refetch** from store | no | yes (CoW source) |
| **Guest-memory** | linear memories, funcref tables | RW (mem), RW→RO (tables) | CoW from Image, else zero; demand-commit | **compress-in-RAM, then swap** | **yes — relocatable** | no (CoW-private) |
| **DMA** | driver DMA buffers + descriptor rings | RW, cacheable | frames, IOMMU-mapped | **none — pinned** | no | no |
| **Device** | MMIO registers, IMSIC pages | RW, **non-cacheable** | device physical (not RAM) | none | no | MMIO per-driver / IMSIC global |
| **Task** | CM callback-task continuations | RW | kernel heap | hot: pinned; **cold-suspended: compress / swap** | yes (heap) | no |

Three corrections to the arena sketch from review, each load-bearing:

1. **Code is *shared*, not CoW, and *recomputed*, not swapped.** Every instance of a
   component maps the same RX code read-only — that is sharing, and there is never a
   copy-on-write event (specialization happens by recompiling to a new tier, not by
   forking code). And code is *recomputable* from the content-addressed component
   bytes, so under pressure the right move is **discard-and-recompile** (like V8
   freeing Liftoff code), which is strictly cheaper than swap — no write-back, the
   "backing store" is the store plus the compiler. Build eviction-by-discard, not a
   code swapper. Size-class packing into hugepages by call-graph locality (§6.2.5): yes.

2. **The guest-memory pool splits, because DMA memory cannot share its policy.** Normal
   linear memory is CoW / compressible / swappable / **relocatable** — the last one is a
   free bonus: guests address memory as `base+bounds-checked-offset` and never see a
   physical address, so the kernel may relocate the physical frames (compaction, hugepage
   defrag) as long as the *virtual base* is stable, and the guest cannot observe it.
   DMA-backed memory (a driver's second linear memory) is the opposite — **wired, IOMMU-
   mapped, never moved/swapped/compressed**, because the device may write it at any moment.
   Same object type (linear memory), two arenas, decided by whether it is DMA.

3. **You cannot swap the swapper.** Swap write-back goes *through a storage component*
   (ZFS-as-library, per the pitch), so the storage path's own memory — disk driver, the
   keyless substrate — must be **pinned**, or a page-out deadlocks trying to page-out.
   This makes "which arena" partly a *placement* decision: a component on the swap path
   lands in a wired arena regardless of its type. It also argues for **compress-in-RAM
   before swap**: compression stays inside the kernel (no component-boundary crossing, no
   pinning dependency) and only genuinely cold pages fall through to the storage-component
   swap path. The Image and Code arenas dodge this entirely — their "swap" is
   refetch/recompile from the store, not a write to it.

**Component-/file-backing** (the reviewer's "backed by a component"): this is the Image
arena. A linear memory's initial content is a component's data segment, held once as a
shared RO image and CoW-mapped into each instance — so backing is by *content hash*, not
a file path, and eviction is refetch-from-store. If demand-*paging* a memory from its
image (rather than eager CoW map) is ever wanted for huge memories, that is the same
Image arena serving faults, and is a later optimization, not v1.

### 3.11 `VirtualAddress` as `isize`, and one address space vs two halves

Two separable questions hide in this idea; they get opposite answers.

**Signed representation — fine, even nice.** Making `VirtualAddress` an `isize` is
reasonable: Sv48 canonicality *is* a sign-extension property (`addr == (addr << 16) >> 16`
arithmetic-shifted is the canonical-form check), and pointer differences / ABI offsets /
PC-relative relocations are naturally signed. Low stakes; adopt it on ergonomics if the
arithmetic reads better. It does not by itself change any layout policy.

**Scattering instances across *both* halves — don't.** The motivation is VA budget, and
that budget is not scarce here: with per-isolate address spaces (§2.7) each isolate owns a
*private* 128 TiB lower half, and at ~6 GiB per full slot that is ~20k full-size memories
*per isolate* — but a machine runs out of physical RAM (20k × 4 GiB = 80 TiB committed)
long, long before it runs out of VA. The binding constraint is frames, not address bits.
Against that non-benefit, using the higher half for guests costs three real things:

1. It **destroys the lower=guest / higher=kernel provenance invariant** (§3.10) — a free
   safety/security check directly serving invariant 5 (keep host and guest provenance
   separate) and the SFI-is-the-TCB posture (§7).
2. It **breaks the isolate-switch optimization** (§3.9): the higher half is the shared
   global kernel precisely so a `satp`+ASID switch reloads only the lower half and kernel
   text/heap/code stay TLB-resident. Put per-isolate guest mappings up there and the
   higher half is no longer global.
3. It buys ~1 extra address bit of randomization entropy — negligible next to
   function-granular ASLR within a 128 TiB half.

So: keep the two-half *policy* (guest lower, kernel higher), and use the signed *type* if
you like it — they are orthogonal. "Treat the whole space as one" is right as a *type*
statement and wrong as an *allocation-pool* statement. If a genuine VA-scarcity workload
ever appears (enormous count of tiny-memory instances in one isolate, packed small-slot),
reach for dense small-slot packing (§2.4) or Sv57 first; both preserve the invariant.

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
6. **TLB economics.** Intra-isolate: no per-call address-space switch, global-bit
   mappings for kernel text and direct map; teardown PTE zaps batched at epoch
   boundaries → `sfence.vma` per epoch, not per unmap, scoped to the harts that ran
   the instance. Cross-isolate switches are ASID-tagged (no flush) and rare (§2.7).
   *Verdict: real; falls out of QSBR + tiered isolation.*
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
6. **Profile infrastructure for free.** Baseline counters (plus, if ever needed,
   call-target feedback for guest-internal indirect calls — the only surviving
   polymorphic sites, see §2.6) give the optimizing tier real PGO on every run
   (wasmtime AOT compiles blind);
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
- **Per-*instance* address spaces.** Re-pay the 25–33% hardware-isolation tax on
  exactly the high-frequency intra-isolate calls the design fuses — the tax lands
  where crossings are cheapest to avoid and blocks fusion/eager-completion. This
  is the granularity error; per-*isolate* address spaces (§2.7) are the opposite
  trade and are in the design.
- **Epochs as the universal clock.** QSBR reclaims; it must not *pace*. Nothing
  latency-sensitive may wait on a grace period (allocation paths stay
  epoch-independent), and backedge checks bound laggard harts.
- **Over-coupling compiler and scheduler.** The full contract between them stays
  exactly two items: backedge epoch checks and placement/edge labels. A compiler
  that knows more about the scheduler (or vice versa) welds shut the seams that
  make the rest of this document's deletions possible.

## 7. Security model and the isolation TCB

Researched after review. The literature is unanimous on one point, and the design
must be built on it rather than against it.

### 7.1 What SFI does and does not buy

**SFI gives host-from-guest *integrity*, contingent on a correct compiler, plus a
*raised bar* on side channels. It never gives Spectre-class confidentiality.** V8's
team ("Spectre is here to stay", 2019) showed untrusted in-process code can build a
*universal read gadget* reading all co-mapped memory via microarchitectural channels,
and concluded there is no comprehensive software mitigation — which is why Chrome
moved to per-site *process* isolation. Measured leak rates frame the threat: >1 KB/s
(C++ v1 gadget with `rdtsc`), >10 B/s (JS, coarse timer); Cloudflare's DyPrIs
demonstrated a *remote* Spectre leak against production Workers at ~120 bit/h even
after freezing timers. Of the transient-execution family, only **v1
(bounds-check bypass)** has a credible in-domain software mitigation (index masking,
SLH at 10–50%, Swivel's linear blocks at ≤10.3% SFI / ≤6.1% CET). **v2/BTI, RSB/
ret2spec, and MDS/L1TF do not** — they need hardware (CET/IBRS-class) or, for MDS,
*disabling SMT*, because MDS leaks across hyperthreads independent of address.

The consequence is not "give up" — Cloudflare and Fastly run millions of
mutually-distrusting SFI tenants — but that they do so on a **layered** stack, never
SFI alone: memory-safe wasm + a *verified* compiler, environmental throttles (no fine
timers, no attacker concurrency), fresh guard-separated instances, a hardware
fallback at tenant granularity, and co-residency control. k23 gets several of these
structurally rather than bolted on (§7.3), which is the real security story here.

### 7.2 The JIT is the entire TCB — treat it as such

With SFI as the boundary, the compiler *is* the isolation mechanism: a single
miscompiled bounds check is a full host compromise. This is not hypothetical —
Cranelift has shipped sandbox-escape miscompiles (CVE-2023-26489: a 35-bit effective
address instead of 33-bit, reaching ~6–34 GB past the base; April 2026 advisories:
an aarch64 heap-access bug where the checked and loaded addresses differed, and Winch
baseline bugs). Two implications specific to this project:

1. **We are writing a *second* compiler** (the riscv64 baseline tier). It will not
   have Cranelift's fuzzing-years behind it. Baseline backends are exactly where the
   April 2026 Winch escapes were. So: differential fuzzing (`wasm-smith`, baseline vs
   Cranelift vs a reference) is a day-one deliverable, not a maturity nicety, and the
   hosted backend (§2.5) is what makes it runnable at scale off-target.
2. **Adopt compiler verification as it matures.** VeriWasm (sound static verifier of
   SFI in compiled output; deployed at Fastly) and Crocus/VeriISLE (SMT verification
   of Cranelift ISLE lowering rules, ASPLOS 2024) are the state of the art. The
   translation-validation shape — verify the *output* per compile — fits a kernel
   that already validates at the store `add` gate; a verifier pass over freshly
   compiled code before it is published (RX-flipped) is the natural hook, and QSBR
   already gates publication.

Also inherited: **intra-component memory is unprotected** ("Everything Old is New
Again", USENIX Sec 2020 — wasm linear memory has no internal guard pages, so a
component compromised *internally* is a stepping stone even though it cannot escape
the sandbox). This does not break the isolation story but it bounds what "one
component = one blast radius" means: a corrupted component can misuse every handle it
holds, so least-authority handle granting (the pitch's model) is a security control,
not just an aesthetic.

### 7.3 What k23 gets structurally, and where the hardware boundary goes

The production pattern the literature converges on — **SFI fast path + hardware
boundary at trust granularity + environmental throttles** — maps onto the existing
design almost exactly, and in several places k23 gets by construction what
Cloudflare/Fastly bolt on:

- **Environmental throttles are structural.** No shared memory between components (no
  `SharedArrayBuffer`-equivalent to build a counting-thread timer), cooperative
  single-threaded execution per instance (no attacker concurrency), and clock
  exposed only as a coarse host-updated import (§6.2.9) — this is precisely
  Cloudflare's timer/concurrency model, except it falls out of the component ABI and
  cooperative scheduler instead of being a special case.
- **The isolate is the hardware-boundary granularity, by declaration not detection.**
  DyPrIs promotes *suspected* workers to processes using runtime HPC monitoring
  because a cloud can't know trust ahead of time. k23 *does* know: the edge trust
  labels (§ pitch) mark exactly which boundaries leave an isolate. So the per-isolate
  address space (§2.7) is DyPrIs made static and precise — the hardware boundary is
  placed where the build says trust changes, with no detector to evade. This is the
  single most important defense-in-depth lever and the reason §2.7 exists.
- **Co-residency control is already in the scheduler.** Home-hart instances + the
  placement labels (§6.2.10) are where "never co-schedule distrusting components on
  one physical core" is enforced — the only real control against MDS/SMT leakage. On
  SMT hardware this must mean SMT-off across trust boundaries, or physical-core
  exclusion for cross-isolate pairs. State it as policy, enforce it in placement.
- **Mitigation by label, not globally.** Speculation barriers, index masking, and
  BTB-flush-on-entry are emitted only on trust-crossing edges by the compiler, which
  knows the labels; intra-isolate fused calls pay nothing. Conventional systems
  mitigate globally because they lack the boundary information we have as data.

### 7.4 RISC-V specifics and the honest core-dependence

The threat is **microarchitecture-dependent in a way that matters for k23's targets**:

- **In-order cores (SiFive U74, Rocket)** do not do the out-of-order speculation that
  drives classic data-cache Spectre v1/v2 — on such a core much of §7.1 is simply not
  exploitable. **Out-of-order cores (SiFive P670, Ventana Veyron, XiangShan,
  Tenstorrent Ascalon)** speculate and are Spectre-v1 territory — demonstrated on
  BOOM and on XiangShan V2/V3 via Flush+Reload; Linux shipped riscv Spectre-v1
  patches in late 2025. So the security/performance trade has an *architectural* knob
  most runtimes don't get to think about: the same k23 image is materially safer on
  an in-order core and faster on an OoO one, and the placement layer could even treat
  "in-order core" as a property to schedule sensitive isolates onto.
- **Zicfilp/Zicfiss (ratified CFI)** give landing pads + a hardware shadow stack —
  the RISC-V equivalent of Intel CET, and the primitive a Swivel-CET-style hardening
  of the JIT's control flow would build on. Important limit: they are *architectural*
  CFI and **do not stop speculation** — a misspeculated indirect branch still
  transiently reaches a gadget before the landing-pad check retires. Use them to
  harden the JIT-as-TCB against architectural control-flow hijack (real value given
  §7.2), not as a Spectre fix.
- **`fence.t.s`** (temporal-partitioning fence, ~1% reported overhead) is the
  promising primitive for cheap domain-switch state clearing on OoO cores, but is
  *unratified* — watch it; don't depend on it.
- **No ratified RISC-V IBRS/STIBP/SSBD-equivalent CSRs** exist yet, so v2 mitigation
  today is software (fences, masking) — reinforcing "in-order core or accept the
  residual."

### 7.5 Beyond speculation

- **Rowhammer: SFI does nothing, and a wasm component is an ideal hammering engine**
  (tight, predictable memory access — Rowhammer.js precedent). This needs a separate
  layer (ECC, target-row-refresh, refresh management) and must be listed as an
  explicit out-of-scope-for-SFI hazard, not silently assumed away.
- **DMA/IOMMU for driver components:** a miscompiled or internally-compromised driver
  that programs DMA can read arbitrary physical memory without an IOMMU domain per
  device — and naive IOMMU mappings still expose page-adjacent data (ASPLOS'16). The
  §3.6 DMA-as-resource design must pair with per-device IOMMU domains and
  copy-vs-map discipline on sub-page buffers.

### 7.6 Net position for k23

SFI is the right *fast-path* boundary and a correct integrity mechanism, and the
design's structure (declared trust labels, isolate address spaces, cooperative
no-shared-memory execution, placement control) gives a genuinely strong layered
posture — arguably better than the retrofit stacks at Cloudflare/Fastly because the
trust boundaries are build-time data rather than runtime guesses. But three things are
non-negotiable and must be stated as such in v1: **(a)** the compiler is the TCB —
differential fuzzing + output verification from day one, doubly so for the hand-
written baseline; **(b)** the per-isolate hardware boundary and cross-trust core
exclusion are the load-bearing Spectre/MDS defenses — "SFI only, one address space,
share cores freely" is *not* a defensible multi-tenant posture on an OoO core; **(c)**
Rowhammer and DMA are outside SFI's remit and need their own layers. None of these
contradict the performance thesis — they land on the rare, coarse, already-semantic
boundaries — but pretending SFI alone is a security boundary would be the one mistake
that discredits the whole approach.

## 8. Risks

- **Spec flux.** Stackful lifts, threads, stream splicing, caller-supplied buffers,
  and the CM-1.0 "lazy value handle" ABI redesign are all in motion. Mitigation:
  isolate lift/lower behind one module; everything cut in §4 is also everything in
  flux.
- **Spectre / transient execution.** Now treated in full in §7. Summary: SFI is not
  a Spectre boundary; the load-bearing defenses are the per-isolate hardware boundary
  (§2.7), cross-trust physical-core exclusion, and label-scoped barriers — all on
  rare coarse edges. The one-address-space-share-cores-freely posture is indefensible
  on an out-of-order core; on an in-order core the classic channels largely evaporate.
- **Guest toolchain maturity.** All-callback-only means p3-native guests only; the
  practical question is how good `wit-bindgen`'s async Rust output is today. Needs a
  spike before committing the fiber-free constraint.
- **Baseline compiler scope.** A riscv64 single-pass compiler is multi-month work.
  The lazy-first sequencing de-risks it; the dispatch-slot contract must be designed
  so the baseline drops in without touching the runtime.
- **Cross-component sync-call overhead.** Wasmtime's 3.5x async-bookkeeping
  regression on sync calls is the cautionary tale; task-state layout must let fused
  sync paths keep task state on the native stack.

## 9. Selected sources

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
- V8, "Spectre is here to stay" (arXiv 2019): <https://arxiv.org/abs/1902.05178>; "A year with Spectre" (leak-rate numbers): <https://v8.dev/blog/spectre>
- Cloudflare Workers security model: <https://blog.cloudflare.com/mitigating-spectre-and-other-security-threats-the-cloudflare-workers-security-model/>; Dynamic Process Isolation (ESORICS 2022 / arXiv 2110.04751): <https://arxiv.org/abs/2110.04751>
- Swivel, Spectre hardening for SFI wasm (USENIX Sec 2021): <https://www.usenix.org/conference/usenixsecurity21/presentation/narayan>
- LLVM Speculative Load Hardening: <https://llvm.org/docs/SpeculativeLoadHardening.html>
- "Everything Old is New Again: Binary Security of WebAssembly" (USENIX Sec 2020): <https://www.usenix.org/conference/usenixsecurity20/presentation/lehmann>
- VeriWasm (NDSS 2021): <https://cseweb.ucsd.edu/~dstefan/pubs/johnson:2021:veriwasm.pdf>; Crocus/VeriISLE Cranelift verification (ASPLOS 2024): <https://cfallin.org/pubs/asplos2024_veri_isle.pdf>
- Wasmtime security advisories (Cranelift/Winch escape CVEs): <https://bytecodealliance.org/articles/wasmtime-security-advisories>; CVE-2023-26489: <https://github.com/advisories/GHSA-ff4p-7xrq-q5r8>
- RISC-V CFI (Zicfilp/Zicfiss): <https://docs.riscv.org/reference/isa/v20260120/unpriv/unpriv-cfi.html>; `fence.t.s` temporal fence (arXiv 2409.07576): <https://arxiv.org/abs/2409.07576>
- Spectre on out-of-order RISC-V: BOOM (CARRV 2019): <https://carrv.github.io/2019/papers/carrv2019_paper_5.pdf>; XiangShan (EuroSec 2026): <https://dl.acm.org/doi/10.1145/3803525.3804986>
- Cage, ARM MTE wasm sandboxing (CGO 2025): <https://arxiv.org/abs/2408.11456>; ERIM, MPK domains (USENIX Sec 2019): <https://www.usenix.org/system/files/sec19-vahldiek-oberwagner_0.pdf>
- x86 PTI/KPTI overhead: <https://docs.kernel.org/arch/x86/pti.html>
