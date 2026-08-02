# A NUMA-first async runtime for k23 — research and design notes

Working notes for replacing `kasync`. Research synthesis first, recommended
design second. Nothing here is final; the point is to make every design choice
against the strongest known prior art instead of folklore.

The framing constraint, restated: we are not a generic scheduler. We schedule
exactly two classes of work — kernel futures (background/misc) and Wasm guest
tasks — we own the JIT, the timer interrupt, the trap handler, and the memory
map. Every place where a userspace runtime has to fight its host, we get to
cheat. Simple is fast; complexity has to buy its way in.

---

## 1. TL;DR — the recommended shape

1. **One task abstraction, one scheduler.** Kernel futures and guest tasks are
   both `NonNull<TaskHeader>`; the difference lives entirely in the poll
   function and the scheduling class byte. No second scheduler — every system
   surveyed that had two converged back to one (§10).
2. **`spawn_raw(NonNull<TaskHeader>)` is validated prior art**, not a novelty:
   Embassy (`TaskRef`), maitake (`TaskRef(NonNull<Header>)`), and tokio
   (`RawTask = NonNull<Header>`) all already work this way internally. We just
   make it the *only* API. Header-first `#[repr(C)]`, type erasure through a
   3-entry vtable.
3. **The raw waker is the `NonNull<TaskHeader>`** with a no-op clone/drop —
   *no reference counting*. Embassy proves this sound when the runtime owns
   task lifetime; QSBR is exactly the mechanism that extends that soundness to
   dynamically allocated tasks on multicore (§3.2). This deletes the entire
   tokio/maitake refcount state machine.
4. **Task memory is reclaimed through `qsbr::Local::retire`**, driven by the
   executor loop. BEAM has run this exact design (scheduler loop reports
   thread-progress, lock-free structures freed at a later progress value) in
   production for two decades.
5. **Const-size everything by `MAX_CPUS` / `MAX_NODES`.** `qsbr::Domain<MAX_CPUS>`,
   `[PerCpu; MAX_CPUS]`, `CpuSet` idle/searching masks, per-node injectors —
   all `static`, zero boot-time allocation for the runtime core. The unused
   `CpuSet` was built for this.
6. **Topology by construction** (Seastar): share-nothing *across* NUMA nodes,
   work-stealing *within* a node. Cross-node work movement only by spawn
   placement and (later) a periodic rebalance plan — never by stealing. No
   production Rust runtime is NUMA-aware today; the literature is unanimous on
   this hierarchy and nobody has shipped it in async Rust. That's our lane.
7. **Per-CPU queue structure:** one non-stealable-but-flushed LIFO slot
   (Go `runnext` with slice inheritance + forced-preempt backstop), one
   fixed-size stealable ring (tokio packed-head, 256 slots), one MPSC remote
   queue (embassy `TransferStack`) for cross-CPU wakes, drained per tick.
8. **Idle protocol:** publish sleeping state in the `CpuSet` idle mask →
   re-check all queues → `qsbr enter_idle` → WFI. Wakers check the mask and
   send one SBI IPI only when the target sleeps. Poll-before-sleep window
   sized to the measured WFI+IPI round trip (Seastar's rule), not guessed.
9. **Priorities:** a small fixed set of scheduling classes per CPU with
   EEVDF-shaped (weight, slice) semantics — not strict priorities, not a
   general priority queue. Class lives in the task header; the wake path never
   needs to know about it.
10. **Guest preemption = epoch interruption owned by the timer trap.** The
    timer interrupt *is* `increment_epoch()`; the same check that bounds guest
    preemption latency bounds QSBR grace-period latency. Callback-ABI
    (stackless) component-model tasks are the preferred guest form: a waiting
    guest task holds *no stack*, so a guest task is an ordinary future.

---

## 2. Starting position (in-tree, as of `508f240`)

Treat all of this as changeable; listed only so the design knows its ground.

Already built and staged for this work:

- `lib/qsbr` — const-generic `Domain<N>`, per-thread `Local` with
  `quiescent`/`enter_idle`/`exit_idle`/budgeted `reclaim`, intrusive
  `Reclaimable` trait (cordyceps `Stack`). Loom-tested. **Zero users today**;
  its own docs say the async runtime is supposed to drive it.
- `sys/cpus` — `MAX_CPUS` (kcfg, default 64), dense `LogicalCpuId`, atomic
  `CpuSet` designed to be updated from trap handlers. **`CpuSet` has zero
  users today.**
- Wasm engine (first-party, Cranelift-based) with epoch-interruption plumbing
  already in place — `Engine::epoch_counter: AtomicU64`, `VMContext::epoch_ptr`,
  `VMStoreContext::epoch_deadline` — but nothing increments or checks it yet.

Facts about the current `kasync` worth keeping in mind while replacing it:

- Multi-worker work-stealing design, but only one worker ever runs (no
  secondary-hart bringup); the enqueue path never wakes a sleeping CPU — the
  moment a second worker exists, a cross-CPU wake can strand a task on a
  sleeping hart's queue.
- Task cancel is `todo!()`; shutdown leaks queued tasks; every task header
  carries a `tracing::Span`.
- Three unrelated CPU numbering schemes coexist (`LogicalCpuId`,
  `Worker::id`, `cpu_local::collection`'s first-touch counter). The new
  runtime should be indexed by `LogicalCpuId` everywhere, full stop.
- Wasm executes synchronously on the worker's native stack; the trap-handler
  activation chain (`cpu_local!` list rooted in stack frames) assumes a task
  is polled to completion of the Wasm call on one stack. Guest suspension
  (fibers) or mid-poll migration would break it — a constraint until that
  subsystem is redesigned too.
- NUMA awareness in-tree: none. QEMU `-numa` config is staged but commented
  out in `build/toolchains/BUCK`. No FDT `numa-node-id` parsing, single global
  frame allocator.

---

## 3. The task: header, waker, lifecycle

### 3.1 What the field looks like

All three serious Rust designs are the same design with different lifetime
management:

| | tokio | maitake | embassy |
|---|---|---|---|
| handle | `RawTask = NonNull<Header>` | `TaskRef(NonNull<Header>)` | `TaskRef(NonNull<TaskHeader>)` |
| layout | `#[repr(C)]` Cell{Header, Core, Trailer}, header first | `#[repr(C)]`, header first, hot fields in header | `#[repr(C)]`, header at offset 0 |
| erasure | 6-entry static vtable in header | split vtables (waker vs task) | single lazily-set `poll_fn` |
| waker data ptr | the header pointer | the header pointer | the header pointer |
| lifetime | refcount packed in state word | refcount packed in state word | **none — static slots, no-op waker drop** |
| queue | 256-ring + LIFO slot + mutex inject list | cordyceps `MpscQueue` (Vyukov, stub node) | cordyceps `TransferStack` (atomic LIFO batch) |
| state bits | RUNNING/COMPLETE/NOTIFIED/CANCELLED/JOIN_* | same family | just SPAWNED + RUN_QUEUED |

Load-bearing details worth stealing verbatim:

- **`NOTIFIED`/`RUN_QUEUED` dedup bit**: set on wake with `fetch_or`, enqueue
  only on the 0→1 edge; clear *before* polling so a wake-during-poll
  re-enqueues. This is what guarantees a task sits in at most one queue —
  it's the entire multi-queue correctness story and it's two `fetch_or`s.
- **Header must be first** so `NonNull<TaskHeader>` and the concrete task
  pointer are interchangeable (`#[repr(C)]`, static-asserted offset 0).
- maitake's **split vtable insight**: waking needs only "re-enqueue on
  scheduler S" — generic over less than polling is. With a single scheduler
  type ours collapses further; a 3-entry vtable suffices:
  `poll(NonNull<TaskHeader>) -> PollResult`, `drop_future(NonNull<TaskHeader>)`
  (cancellation), `retire(NonNull<TaskHeader>)` (hand to QSBR with the right
  concrete type).
- tokio's `waker_ref` pattern: the *borrowed* waker passed to `poll` never
  touches lifetime management at all. With no-op clone/drop this becomes
  moot, which is the point.

### 3.2 QSBR-managed lifecycle — the core move

The problem refcounting solves: a `Waker` is `Send + Sync + 'static` and may
be cloned into places that outlive the task's execution; whoever drops the
last clone must free the task, and concurrent wake-vs-complete races must not
use-after-free. Tokio/maitake pay for this with a refcount in the state word
and a genuinely hairy set of protocols (tokio's `JOIN_WAKER` seqlock, the
current `kasync` busy-wait handshake that defeats loom).

Embassy shows the alternative: if the runtime owns task lifetime, waker
clone/drop are no-ops and `wake` is `fetch_or + maybe-enqueue`. Embassy gets
ownership from static pools (tasks are never freed). We get it from QSBR:
tasks are freed, but only after a grace period in which every CPU has passed
through the executor loop.

**Soundness argument.** A stale `NonNull<TaskHeader>` (from a waker copy or a
queue traversal) is dereferenceable iff the header memory hasn't been freed.
Retirement flows through `qsbr::Local::retire`, and reclamation waits until
every non-idle CPU has passed a quiescent state (the top of the executor
loop). Therefore any code that (a) obtained the pointer while the task was
live and (b) runs entirely between two quiescent states of its CPU may
dereference freely. Wakes from task context, from the timer wheel, from wait
queues, and from trap handlers (which are bracketed inside one loop iteration
of the interrupted CPU) all satisfy (b). A wake that races completion sees
the state word's `COMPLETE` bit and no-ops — the memory is still there for
the entire grace period.

**The escaped-waker hazard — the one rule.** QSBR read sections cannot span
`.await`, and neither can this argument: a `Waker` *cloned and stashed
somewhere that outlives the grace period* (a `static`, a long-lived struct on
another task) would dangle. Three-part answer:

1. **Intrusive wait primitives don't store `Waker` at all.** WaitCell/
   WaitQueue/timer entries live *inside the waiting future*, i.e. inside the
   task allocation itself; the future's `Drop` (which runs before retirement,
   per cancellation-safety invariant 6) unlinks them. After the future is
   dropped, those registrations are gone; a racing waker on another CPU has
   its access covered by the grace period. This is already the house style
   (maitake-sync) — it becomes a soundness requirement instead of a
   performance preference.
2. **Wake paths outside task context are naturally bracketed.** Trap handlers
   and the executor's own machinery run within one loop iteration.
3. **`Waker::clone` is the escape hatch and gets policed.** Two options:
   (a) forbid it — clone panics/aborts in debug, kernel code uses only
   blessed primitives; (b) make clone fall back to a real refcount (a small
   count in the state word used *only* by escaped clones; the common borrowed
   path never touches it). Recommendation: start with (a) — we own all the
   code; grep is a viable enforcement strategy in a kernel — and add (b) only
   if a legitimate need appears. Either way `wake_by_ref` and the poll-time
   borrowed waker are always free.

What QSBR buys beyond wakers, same mechanism, no extra machinery:

- Lock-free run-queue nodes and any shared runtime tables.
- **JIT lifecycle**: unloading a module/instance while another CPU may still
  be executing its code or reading its `VMContext` is exactly a
  retire-after-grace problem. The "runtime exposes its `Local` read lock as a
  service" idea makes every kernel subsystem's lock-free reads free.
- Guest instance teardown (memories, tables) against concurrent host-side
  readers.

**Grace-period latency = preemption latency.** A CPU running a guest reports
quiescence only when the guest yields; the epoch check that bounds preemption
(§8.2) therefore also bounds grace periods. One mechanism, two guarantees —
this is the BEAM shape (`CONTEXT_REDS = 4000` bounds both descheduling and
thread-progress). Idle CPUs are `enter_idle` and never hold anything back;
that's already in `lib/qsbr`.

### 3.3 Recommended `TaskHeader`

```rust
#[repr(C)]
pub struct TaskHeader {
    /// SPAWNED | RUN_QUEUED | COMPLETE | CANCELLED | class:2 …
    /// single AtomicUsize; RUN_QUEUED 0→1 edge gates every enqueue.
    state: State,
    /// intrusive link for whichever run/remote queue currently owns the task.
    run_link: /* links for the chosen queue types */,
    vtable: &'static TaskVTable,   // poll, drop_future, retire
    /// home CPU (dense LogicalCpuId); wake pushes here, stealing rebinds it.
    home: AtomicU32,
    qsbr: qsbr::Links,             // reclamation hook
}

pub struct TaskVTable {
    poll:        unsafe fn(NonNull<TaskHeader>) -> Poll<()>,
    drop_future: unsafe fn(NonNull<TaskHeader>),
    retire:      unsafe fn(NonNull<TaskHeader>, &qsbr::Local<'_, MAX_CPUS>),
}

pub unsafe fn spawn_raw(task: NonNull<TaskHeader>) { /* the only spawn API */ }
```

Anything embedding a `TaskHeader` at offset 0 is schedulable: a boxed kernel
future, an arena-allocated guest task record, a static. Join handles, task
IDs, output storage, spans, metadata — all live in the embedder's struct if
that embedder wants them, not in the header. (`JoinHandle` becomes a small
wrapper crate concern built on a WaitCell in the embedding struct; the
runtime core does not know it exists.)

---

## 4. Queues and topology

### 4.1 Per-CPU

Three tiers, in poll order:

1. **`next` slot** — single task, written by "wake while running" (the
   message-passing ping-pong optimization; data is still in cache). Go's
   discipline, adopted wholesale: the slot task *inherits the current time
   slice* rather than getting a fresh one, and the forced-preemption backstop
   (§8.2) breaks ping-pong starvation. Tokio's cap (≤3 consecutive slot polls
   per tick) is a cheaper approximation; either works — pick when we have a
   benchmark. Flushed to the ring on park. Not stealable (tokio's unstealable
   slot is a known footgun *only because* tokio lacks the forced-preempt
   backstop; we have one).
2. **local ring** — fixed 256-slot ring of `NonNull<TaskHeader>`, tokio's
   packed head (real|steal indices in one atomic): owner push/pop with no RMW
   on the fast path, thieves CAS-claim. Overflow spills half to the node
   injector (second half, tokio's argument: don't bounce fresh injector
   arrivals straight back).
3. **remote queue** — MPSC for wakes arriving from other CPUs: embassy's
   `TransferStack` (cordyceps), push = one CAS from anywhere including trap
   handlers, drain-all once per tick. `enqueue` returning `was_empty` is the
   IPI-doorbell trigger (§5).

Everything const-sized: `static RUNTIME: Runtime` containing
`[PerCpu; MAX_CPUS]`, where `PerCpu` is `CachePadded` and contains the three
queues, the `qsbr::Local` slot index, and the timer wheel (§9). Ring slots
are just pointers, so the whole thing lives in `.bss`. Stub nodes for
intrusive MPSC queues are statically allocatable (maitake's
`new_with_static_stub`).

Why not a Chase–Lev deque: the owner-side `SeqCst` fence per pop is the known
cost, and the PPoPP'13 weak-memory recipe is exactly the kind of code
invariant 2 exists to fear on RISC-V (RVWMO is ARM-class weak; this structure
is the canonical "works on x86, corrupts on RISC-V"). Tokio's packed-head
ring is loom-provable and has no owner-side fence. BWoS (OSDI'23,
block-based stealing: owner/thief sync only at block boundaries, 2.68× over
tokio's queue in microbenchmarks) is the upgrade path *if* stealing overhead
ever shows in profiles — it's a drop-in replacement for tier 2, not a
rearchitecture.

### 4.2 Per-node and cross-node

Per NUMA node (`MAX_NODES`, new kcfg constant, default 1):

- **injector** — intrusive Vyukov MPSC (maitake-style consumer-claim
  stealing): spawn overflow, tasks with no home, and rebalance traffic land
  here. All CPUs of the node poll it at a fixed interval of their tick loop
  (Go's `%61`-style prime interval; the auto-tuned EWMA variant from tokio is
  a later refinement).
- **timer wheels** are per-CPU (§9), the QSBR domain is global (`Domain` scan
  is one cache line per 8 CPUs at `MAX_CPUS=64`; a per-node combining tree —
  Linux's `rcu_node`, fanout 16 — only becomes worth it far above that).

Cross-node policy, in order of preference (and implementation order):

1. **Placement**: `spawn` lands on the spawner's node; guest tasks land on
   the node owning their instance's memory. Wakes go to the task's home CPU.
   Placement at wake/spawn time is the main locality tool (Linux
   `wake_affine`), migration the backstop.
2. **No cross-node stealing. Ever.** Seastar's position, Vyukov's Go NUMA
   design doc (per-node hierarchies, work-conservation relaxed at node
   boundaries — never implemented in Go; effectively a free design review),
   and Linux sched-domains (balancing interval and imbalance tolerance grow
   with domain span) all agree.
3. **Periodic rebalance plan** (later, only if a real imbalance workload
   exists): BEAM's `check_balance` — every N ticks *one* CPU computes
   migration paths from queue-length histories; enqueue operations consult
   the current plan for free. Compaction bias (pack load, let whole nodes
   idle — BEAM `+scl`, SPDK dynamic scheduler) rather than spreading.

Within a node: steal-half from a random coprime-ordered victim (Go), only
when the local tiers are empty, throttled by a searching-CPU cap (§5).

### 4.3 Stealing discipline (within node)

- At most `⌈node_cpus/2⌉` simultaneous searchers (Go: `2*spinning < busy`).
  A `CpuSet` per node (`searching` mask) makes the check one atomic load.
- Victim order: random start, coprime stride (Go's `randomOrder`) over the
  node's CPUs — indexed by `LogicalCpuId`, fixing the current `iter().nth()`
  walk over an unrelated numbering.
- Steal-half from the ring; never touch the victim's `next` slot (Go only
  does on the final pass with a 3 µs penance delay; we simply don't — the
  forced-preempt backstop makes slot starvation impossible).
- The "last searcher that finds work wakes one more searcher" rule (Go/tokio
  both) preserves work-conservation without thundering herds. The Go
  `proc.go` "Worker thread parking/unparking" comment is the reference
  spec, including the StoreLoad-barrier submit/park race protocol.

---

## 5. Idle, wakeup, IPIs

State: two global `CpuSet`s (`idle`, per-node `searching`), both updatable
from trap handlers by design.

Park sequence (order matters; this is the classic lost-wake race):

1. flush `next` slot to ring; run a QSBR `reclaim(budget)`;
2. set self in `idle` mask (`AcqRel` RMW — the publish);
3. **re-check** remote queue, ring, injector, timer deadline (Go's
   double-check after declaring parked);
4. `qsbr enter_idle`;
5. program next timer deadline, WFI.

Wake sequence (`wake_one` from anywhere, including trap handlers — no locks,
no allocation, invariant 8 clean):

1. push task to home CPU's remote queue (one CAS);
2. if push observed empty→nonempty *and* home CPU is in `idle` mask: clear
   its bit (the CAS-clear arbitrates concurrent wakers — exactly one sends)
   and `sbi::ipi::send_ipi`. At `MAX_CPUS ≤ 64` a hart mask is one `usize`,
   so even a broadcast is one SBI call — the kcfg doc already makes this
   argument.

The IPI receive side stays a near-nop (clear ssoft); the woken CPU's loop
does `exit_idle` and finds the work itself. Pending-IPI latching makes the
unpark-before-park race safe, as the current `RiscvPark` notes.

**Poll-before-park window:** don't guess. Seastar's rule: spin-poll for a
period competitive with the sleep/wake round trip (their default 200 µs bare
metal, 2 ms virtualized *because* virtualized IPIs are ~10× dearer — and we
run under QEMU, so measure both). Benchmark WFI+IPI+trap-return on our
targets and set the window to a small multiple. Until measured: no spin
window at all (park immediately) — simplest, and correct-if-slower.

This closes the current runtime's real bug (enqueue never wakes anyone) as a
side effect of the design rather than as a patch.

---

## 6. QSBR as a service of the runtime loop

The executor tick, annotated with the QSBR calls (all costs are one relaxed
load + occasional release store):

```text
loop:
    local.quiescent()                 // top of loop = quiescent state
    drain remote queue → ring
    poll timer wheel (expired → wake)
    task = next_slot ∥ ring.pop() ∥ injector ∥ steal
    if none → park sequence (§5: reclaim, enter_idle, wfi, exit_idle)
    poll(task)                        // guest tasks: bounded by epoch (§8.2)
    every N ticks: local.reclaim(budget)
```

The service exposed to the rest of the kernel:

```rust
/// Runs `f` inside this CPU's QSBR read section. Callable from task context.
/// The guard cannot cross `.await` (compile-time: `Guard: !Send` + lifetime).
pub fn read<R>(f: impl FnOnce(&qsbr::Guard) -> R) -> R;
/// Retire an object owned by nobody, freed after a grace period.
pub unsafe fn retire<T: Reclaimable>(node: NonNull<T>);
```

Rules that make it sound (all already argued in §3.2):

- No guard across `.await` — the existing `qsbr::Guard` lifetime design
  already enforces this; every poll-loop iteration is a quiescent state.
- Idle CPUs are QSBR-offline (or one sleeping core stalls every grace
  period — the liburcu/Linux dyntick rule).
- Trap handlers are not read sections (invariant 4's "no locks the
  interrupted code might hold", extended to guards).
- Guest epoch checks bound quiescence latency to the preemption quantum.

BEAM's `erts_thr_progress` is the two-decade production precedent for
precisely this: schedulers report progress between process executions and at
sleep/wake; sleeping schedulers are counted as immediately-progressed; frees
trigger at a later progress value.

---

## 7. Priorities

What the field converged on:

- **Strict priorities starve**; every serious design ends at proportional
  share (Seastar/glommio vruntime over queues, Linux EEVDF lag+deadline,
  BEAM's skip-count for `low`) plus at most a tiny strict tier for truly
  latency-critical work (embassy/RTIC: hardware interrupt priority).
- **Priority attaches to a queue/class, not a task-orderable key** — the wake
  path must never need a priority queue. Glommio: shares per `TaskQueue`,
  min-heap on vruntime *across queues*, FIFO within. Seastar: same, with
  reciprocal-multiply share accounting (`vruntime += runtime * (2³²/shares) >> 32`)
  to avoid division. EEVDF's contribution: express latency preference as
  *slice length*, not weight — small slice ⇒ earlier virtual deadline ⇒
  scheduled sooner but preempted sooner; latency and throughput trade in one
  parameter, starvation-free by lag bounds.

Recommendation, in stages:

- **v1: two classes.** `Kernel` and `Guest`, fixed order of consideration
  with a BEAM-style skip counter so guests can't be starved by a busy kernel
  class (strict-ish, starvation-free, ~10 lines). Class = 2 bits in the
  state word; each class is its own ring behind the shared `next` slot.
- **v2 (when a workload demands it): (weight, slice) per class,** glommio/
  EEVDF-shaped vruntime pick across the per-CPU class array. The per-CPU
  structure is `[Ring; NUM_CLASSES]` either way, so v1→v2 changes the pick
  function only.
- **Never:** per-task dynamic priorities, cross-CPU priority coordination, a
  global priority queue.

The embassy/RTIC "priority = separate executor in a higher interrupt level"
trick has a k23 analog (a micro-executor polled from the trap path for
sub-tick latency work) — file under "only if a real need appears"; it
compromises invariant 8's simplicity (trap handlers polling arbitrary
futures) for latency we haven't yet shown we need.

---

## 8. Wasm guests as tasks

### 8.1 Granularity: component-model tasks, stackless by default

The component-model async design (WASI 0.3, shipped mid-2026; Wasmtime 43+)
answers the "instance or sub-instance?" question for us:

- **Schedule tasks, not instances.** A CM *task* is the per-export-call
  bookkeeping unit; one instance legally hosts many concurrent tasks — the
  instance is admission control (backpressure counter, reentrancy lock), not
  the unit of execution. *Subtasks* are the caller-side records of import
  calls — edges in the wait graph, not schedulable things.
- **The callback (stackless) ABI is the fast path**: a waiting guest task has
  *no live core-wasm frame* — its state is entirely in guest linear memory
  plus a small task record (waitable-set registrations, callback funcref,
  event queue). Poll = invoke the callback with pending events; `Pending` =
  callback returned WAIT/YIELD. **A guest task is therefore an ordinary
  future embedding a `TaskHeader`** — no fiber, no stack switch, trivially
  cancellation-safe, composes with everything above.
- Waitable-sets map onto our wait primitives (they're explicitly modeled on
  epoll); `backpressure.inc/dec` maps onto spawn admission; the supertask
  chain gives structured tracing for free.
- **Stackful-ABI guests and sync guests calling async imports need a fiber**
  (a suspended stack across polls). Wasmtime's `wasmtime-fiber` shows the
  shape (custom stack provider — i.e. our frame allocator — guard page,
  ~register-save+SP-swap switch cost). This is *deferrable*: static ABI
  knowledge makes "never needs a fiber" decidable at load time (a component
  whose exports are callback-ABI and imports are sync-or-stackless never
  suspends mid-stack). v1 supports stackless only; the current synchronous
  path (Wasm on the worker stack, activation-chain trap handling) remains
  valid for fully-sync components. Fibers are an additive later feature, and
  the activation-chain redesign lands with them.

### 8.2 Preemption: we own the JIT and the timer

Wasmtime's epoch interruption, upgraded by kernel ownership:

- Timer trap handler: `engine.epoch.fetch_add(1, Relaxed)` — the entire
  cross-CPU preemption mechanism is one atomic increment in an interrupt we
  already take. No signals, no watchdog threads.
- JIT emits at loop backedges + function entries: load deadline via
  `VMContext` (plumbing already exists in-tree), compare, branch-to-yield.
  Wasmtime measured epochs ~2× cheaper than fuel; the check is two dependent
  loads — ownable improvements: pin the epoch word to a `tp`-relative slot
  (one load), or BEAM-style keep the budget in a reserved register.
- Yield path = the task returns `Pending` + self-wake to the back of its
  class ring; deadline bumped by one slice. This is also the quiescence
  bound (§6) *and* the `next`-slot starvation backstop (§4.1): one mechanism,
  three jobs.
- Compiler-enforced max check spacing gives a *hard* bound on preemption
  latency (BEAM's guarantee, at epoch cost). Trap-handler tricks (resume-at-
  safepoint via Cranelift metadata, patching loop-header nops) are documented
  possibilities the design keeps open, not v1 work.
- Known hole, inherited from every engine: epochs don't interrupt a guest
  parked in a *host* call — host imports on guest paths must themselves be
  async/cancellable. That's an import-design rule, not scheduler machinery.
- BEAM's yield-fragment lesson for the JIT: emit the spill-and-exit sequence
  once, shared, not per function.

### 8.3 One scheduler or two?

One. The two workloads differ in:

- poll implementation (Rust future vs enter-JIT-until-yield) — absorbed by
  the vtable;
- preemption mechanism (coop budget vs epoch) — both are "poll returns
  `Pending` on a budget", absorbed by the poll implementation;
- scheduling class (kernel vs guest) — 2 bits in the header (§7);
- lifecycle (join handle vs instance teardown) — lives in the embedding
  struct, invisible to the runtime.

Nothing left over justifies a second dispatch loop, second queue hierarchy,
second idle protocol, and second set of loom models. (Precedent: Seastar
schedules I/O continuations and compute in one reactor; BEAM schedules ports
and processes in one run queue system; Go schedules everything as a G.)

---

## 9. Timers

Per-CPU hierarchical wheel (the current 6×64 bitmap design is fine — it's
maitake's, which is Linux's), with maitake's ISR discipline: an atomic
`pending_ticks` outside the lock; `try_turn` never spins (trap-safe),
`turn` from the owner loop. Per-CPU wheels fix the standing FIXME (#490:
`time`/`stimecmp` are hart-local) by construction — each CPU programs its own
`stimecmp` from its own wheel's next deadline; no global timer lock, no
cross-CPU clock skew problem inside one wheel. `Sleep` entries stay intrusive
in the awaiting future (cancellation-safe via `PinnedDrop`, as today). A
timer armed from CPU A for a future homed on CPU B is just a remote wake at
expiry — no cross-CPU wheel access.

---

## 10. Deliberate non-goals (v1)

- Cross-node stealing, rebalance planning, compaction (§4.2 stage 3).
- Fibers / stackful guest tasks (§8.1).
- (weight, slice) proportional share (§7 v2) — two classes first.
- BWoS rings, EWMA-tuned injector intervals, spin-poll windows — measured
  upgrades, each behind a benchmark.
- Task metadata, spans, IDs in the header — embedder concerns.
- A public `spawn(future)` convenience API can wrap `spawn_raw` + `Box` in
  five lines; the runtime core knows only `NonNull<TaskHeader>`.

## 11. Open questions

1. **`MAX_NODES` and node discovery.** New kcfg int + FDT `numa-node-id` /
   `distance-map` parsing + `LogicalCpuId → node` table + per-node frame
   arenas. The QEMU `-numa` lines in `build/toolchains/BUCK` need
   uncommenting for any of this to be testable. Sizeable, independent
   workstream — the runtime just consumes the table.
2. **Escaped-waker policy** (§3.2): forbid `Waker::clone` outright vs
   refcount fallback for clones only. Start forbidden; revisit at the first
   legitimate use.
3. **`JoinHandle` shape**: WaitCell + output slot in the embedding struct is
   the plan; does anything need join-with-output across CPUs before guest
   tasks exist? (The test harness does — it `try_join_all`s task handles.)
4. **Cancellation protocol**: CANCELLED bit + wake + `drop_future` at next
   dequeue is the standard answer; guest tasks additionally get the CM
   cancel event (cooperative) with epoch-deadline kill as the backstop.
   Needs a precise state diagram before implementation.
5. **How much of `maitake-sync` to keep** vs re-derive on the no-refcount
   waker: WaitCell/WaitQueue store `Waker` by value today; with waker =
   pointer + no-op clone this is fine as-is (a stored waker is just a stored
   pointer) — but the wake-transfer-on-cancel discipline must be preserved.
6. **Loom/miri coverage plan**: the dedup-bit enqueue, park/wake race, ring
   steal, and QSBR-retire-vs-wake races each need models; this is the actual
   acceptance gate for the whole design (per repo policy: property/loom over
   example tests).

---

## 12. Sources

Runtimes: [tokio scheduler blog](https://tokio.rs/blog/2019-10-scheduler) ·
[tokio queue.rs](https://github.com/tokio-rs/tokio/blob/master/tokio/src/runtime/scheduler/multi_thread/queue.rs) ·
[tokio LIFO-slot issues #4323](https://github.com/tokio-rs/tokio/issues/4323)/[#4941](https://github.com/tokio-rs/tokio/issues/4941) ·
[embassy-executor raw](https://github.com/embassy-rs/embassy/blob/main/embassy-executor/src/raw/mod.rs) ·
[maitake](https://github.com/hawkw/mycelium) (scheduler, steal, task, timer wheel, maitake-sync) ·
[glommio](https://www.datadoghq.com/blog/engineering/introducing-glommio/) ·
[monoio](https://github.com/bytedance/monoio) ·
[async-executor](https://github.com/smol-rs/async-executor) ·
[RTIC 2](https://rtic.rs/2/book/en/) ·
[without.boats: thread-per-core](https://without.boats/blog/thread-per-core/).

Systems: Seastar [reactor.cc](https://github.com/scylladb/seastar/blob/master/src/core/reactor.cc)/[scheduling.hh](https://github.com/scylladb/seastar/blob/master/include/seastar/core/scheduling.hh)/[smp.hh](https://github.com/scylladb/seastar/blob/master/include/seastar/core/smp.hh) ·
Go [proc.go](https://github.com/golang/go/blob/master/src/runtime/proc.go) (parking/unparking comment; `runnext`; `%61`) ·
[Vyukov Go NUMA design doc](https://docs.google.com/document/d/1d3iI2QWURgDIsSR6G2275vMeQ_X7w-qxM2Vp7iGwwuM/pub) ·
BEAM [erl_process.c](https://github.com/erlang/otp/blob/master/erts/emulator/beam/erl_process.c) / [erl_thr_progress.c](https://github.com/erlang/otp/blob/master/erts/emulator/beam/erl_thr_progress.c) / [BEAM book scheduling](https://github.com/happi/theBeamBook/blob/master/chapters/scheduling.asciidoc) ·
[EEVDF (LWN)](https://lwn.net/Articles/925371/) ·
[sched_ext / scx_lavd](https://github.com/sched-ext/scx) ·
[ghOSt (SOSP'21)](https://cs.stanford.edu/~jhumphri/documents/ghost.pdf) ·
[Shenango (NSDI'19)](https://www.usenix.org/system/files/nsdi19-ousterhout.pdf) ·
[Caladan (OSDI'20)](https://amyousterhout.com/papers/caladan_osdi20.pdf) ·
[BWoS (OSDI'23)](https://www.usenix.org/system/files/osdi23-wang-jiawei.pdf) ·
[Chase–Lev on weak memory (PPoPP'13)](https://fzn.fr/readings/ppopp13.pdf) ·
[HotSLAW hierarchical stealing](https://upc.lbl.gov/publications/MinEtAl-HotSLAW-Work-Stealing-PGAS11.pdf) ·
[µs-scale scheduling policies (NSDI'22)](https://amyousterhout.com/papers/scheduling_policies_nsdi22.pdf) ·
[Vyukov: task scheduling strategies](https://www.1024cores.net/home/scalable-architecture/task-scheduling-strategies) ·
[Linux RCU requirements](https://github.com/torvalds/linux/blob/master/Documentation/RCU/Design/Requirements/Requirements.rst) ·
[liburcu QSBR](https://liburcu.org/).

Wasm/JIT: [Wasmtime epoch interruption PR #3699](https://github.com/bytecodealliance/wasmtime/pull/3699) ·
[wasmtime-fiber](https://github.com/bytecodealliance/wasmtime/blob/main/crates/fiber/src/lib.rs) ·
[component-model Concurrency.md](https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md) ·
[WASI 0.3 launch](https://bytecodealliance.org/articles/WASI-0.3) ·
[stack-switching explainer](https://github.com/WebAssembly/stack-switching/blob/main/proposals/stack-switching/Explainer.md) ·
[BeamAsm internals](https://github.com/erlang/otp/blob/master/erts/emulator/internal_doc/BeamAsm.md) ·
[Go preempt.go](https://github.com/golang/go/blob/master/src/runtime/preempt.go) ·
[workerd io-context.h](https://github.com/cloudflare/workerd/blob/main/src/workerd/io/io-context.h) ·
[Wasmachine (PerCom'20)](https://staff.itee.uq.edu.au/jaga/proceedings/percomworkshops2020/papers/p343-wen.pdf).
