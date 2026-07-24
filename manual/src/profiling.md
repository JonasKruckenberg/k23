# Whole-System Profiling & Benchmarking (Design)

> Status: **design proposal**, not yet implemented. This document lays out the
> architecture for observing where time goes in the *assembled* system — kernel
> and WebAssembly guest together — and for benchmarking the whole stack rather
> than components in isolation.

k23 already microbenchmarks individual `lib/` crates in isolation (criterion via
[`build/bench.bzl`], run with `just benchmark`). That answers "is this data
structure fast?" It does *not* answer the questions this document is about:

- Where does time actually go when the kernel is running a real WASM workload?
- Which spans / subsystems / guest functions dominate a given run?
- How do we catch a whole-system performance regression in CI?

The goal is one coherent pipeline — a single time base, a single per-CPU event
buffer, two producers (instrumentation and sampling), and one export path — that
covers **both** kernel and guest code and renders in [Perfetto].

[`build/bench.bzl`]: https://github.com/JonasKruckenberg/k23/blob/main/build/bench.bzl
[Perfetto]: https://perfetto.dev

## Design principles

1. **One time base.** Everything timestamps from the RISC-V `time` CSR
   (`riscv::register::time::read64()`), the same source the async runtime's
   `Clock` already uses (`sys/kernel/src/arch/riscv64/device/clock.rs`).
2. **Two producers, one buffer.** Instrumentation (span enter/exit) and
   sampling (timer-ISR stack captures) both write into the *same* per-CPU,
   lock-free ring buffer, tagged by kind. This is the Fuchsia/`ftrace` shape.
3. **Aggregate early, export lazily.** Where possible, summarize in-kernel
   (histograms) — DTrace's model — to keep the observer effect and serial
   bandwidth low. Symbolize offline on the host.
4. **Zero cost when off.** Gate everything behind a build feature (mirroring the
   existing `counters` / `__bench` features) *and* a runtime arm/disarm via the
   shell. The default kernel build pays nothing.
5. **Respect the critical invariants** (see `CLAUDE.md`). In particular the
   sampling path runs inside a trap handler and must not allocate, panic, or
   take a lock the interrupted code holds (invariants 4, 7, 8).

## What already exists (the foundation)

Most of the hard primitives are already in the tree; they just aren't wired into
a profiling pipeline.

| Primitive | Location | Role |
|---|---|---|
| Monotonic clock (`time` CSR) | `sys/kernel/src/arch/riscv64/device/clock.rs:24`; `Clock`/`Timer`/`Instant` in `sys/async/src/time/` | Timestamp base. Frequency from devicetree `timebase-frequency`. |
| Custom `tracing_core` subscriber + span `Registry` | `sys/kernel/src/tracing/mod.rs` (`enter`/`exit`/`new_span`/`try_close` hooks) | Span lifecycle hooks exist but record **no timing** — `record` is a `// TODO` (`mod.rs:213`). |
| Executor runtime metrics | `sys/async/src/executor.rs:44` — `Tick { polled, completed, spawned, woken_external, woken_internal }`, behind the `counters` feature (already enabled: `sys/async/BUCK:32,48`) | Tokio-`RuntimeMetrics`-style data, computed every tick, currently discarded. |
| Timer interrupt | `sys/kernel/src/arch/riscv64/trap_handler.rs:300` (`Interrupt::SupervisorTimer` arm) | Sampling trigger. |
| Stack capture from a trap frame | `sys/backtrace/src/lib.rs:132` — `Backtrace::from_registers(regs, pc)`, documented cheap + lazily symbolized | Sampling profiler ISR primitive. The handler already receives `frame: &mut unwind::Registers` (`trap_handler.rs:265`). |
| Guest PC → wasm module | `sys/kernel/src/wasm/code_registry.rs:22` — `lookup_code(pc) -> (CodeObject, text_offset)` | Symbolizes sampled guest PCs. |
| Frame pointers in JIT'd guest code | `sys/kernel/src/wasm/engine.rs:44` — `preserve_frame_pointers = "true"` | Makes guest frames FP-walkable → unified kernel+guest stacks feasible. |
| Extensible kernel shell | `sys/kernel/src/shell.rs:29` (`COMMANDS` table, runs as a spawned task) | Control surface: arm / dump / reset profiling. |
| Host microbench harness | `build/bench.bzl`, `just benchmark` | Isolated `lib/` crates only; host-only. |

## Precedent

- **Linux** splits observability in two: **ftrace** (static tracepoints +
  function-graph → per-CPU ring buffer, gives *durations*) and **perf**
  (PMU/timer sampling → stack captured on interrupt overflow, symbolized offline
  → flame graphs). Two producers, per-CPU ring, offline symbolization.
- **DTrace (illumos/BSD)** — the reference for *safe, production, whole-system*
  tracing. Two ideas we borrow: its `profile-N` provider is just a timer-driven
  stack sampler, and it *aggregates in-kernel* so only summaries leave. Its
  "a probe must never panic the kernel" rule is our invariant 8.
- **Zircon/Fuchsia** — closest to k23's architecture. Static instrumentation
  (`TRACE_DURATION`, categories) *and* kernel `ktrace` feed **one per-CPU
  buffer**, drained by a trace manager, exported as FXT and viewed in
  **Perfetto**. Their CPU profiler samples thread stacks on a timer and
  symbolizes host-side. This design is essentially "k23's tracing subscriber,
  finished, plus a sampler, plus Perfetto."
- **Rust ecosystem** — ready-made patterns: `tracing-timing` (per-callsite
  histograms), `tracing-flame` / `tracing-chrome` (spans → flamegraph / Chrome
  JSON), **tokio-console** (a tracing layer + runtime instrumentation), and
  `pprof-rs` (timer + backtrace → pprof). Most relevant: **Wasmtime's profiler**
  solves our exact hard case — frame-pointer unwinding across the host↔guest
  boundary — emitting `perf` jitdump / Firefox Profiler output for mixed
  native+wasm stacks.

**Conclusion:** adopt the Fuchsia/DTrace shape (one time base → per-CPU ring →
two producers → host converter → Perfetto), aggregating in-kernel where possible.

## Architecture

```
   ┌─────────────────────────┐        ┌──────────────────────────┐
   │ Instrumentation producer │        │   Sampling producer      │
   │ tracing enter/exit → Δt  │        │ SupervisorTimer ISR →    │
   │ (Registry span timing)   │        │ Backtrace::from_registers│
   └────────────┬────────────┘        └────────────┬─────────────┘
                │  (ts, cpu, kind, payload)         │
                ▼                                   ▼
        ┌───────────────────────────────────────────────────┐
        │   per-CPU lock-free ring buffer (pre-allocated)    │  ← lib/ MpscQueue
        │   + in-kernel per-callsite histograms (aggregate)  │
        └───────────────────────┬───────────────────────────┘
                                 │  drain (shell cmd / serial)
                                 ▼
                   ┌───────────────────────────┐
                   │  host converter (offline)  │  symbolize kernel (DWARF)
                   │  compact binary → Perfetto │  + guest (lookup_code)
                   └───────────────────────────┘
                                 ▼
                             Perfetto UI
```

Common record header: `(timestamp_ticks, cpu_id, kind)` where `kind ∈ {span_begin,
span_end, instant, sample}`. Kernel PCs symbolize via the existing backtrace
symbolizer; guest PCs via `lookup_code`.

## Phased plan

### Phase 0 — foundation (small)
- Alloc-free `now_ticks()` reading the `time` CSR directly (skip the `Clock`
  vtable on hot paths).
- Surface the already-computed executor `Tick` counters via a shell `stats`
  command or a periodic tracing event — a near-free runtime-health win.
- Introduce a `profiling` build feature mirroring `counters` / `__bench`.

### Phase 1 — span durations + per-callsite histograms (small; **start here**)
Finish the `// TODO` at `sys/kernel/src/tracing/mod.rs:213`: on `enter` stash
`now_ticks()` in the span's `Registry` extension; on `exit`/`try_close` compute
the delta. Feed an aggregating sink keeping per-callsite min/max/mean/p50/p99/count
(the `tracing-timing` model), dumpable from the shell. Then `#[instrument]` the
existing seams: executor tick & task poll (`sys/async/src/executor.rs`), trap
entry/exit, wasm compile / instantiate / guest-call / host-call boundary /
`memory.grow` (`sys/kernel/src/wasm/`), and page-fault handling. This answers
"precisely where does time go" with no sampler and minimal risk.

### Phase 2 — timer-driven sampling profiler (medium)
In the `SupervisorTimer` arm (`trap_handler.rs:300`), when armed, call
`Backtrace::from_registers(frame_clone, epc)` and push raw frame PCs into a
**pre-allocated per-CPU ring** — no alloc, no panic, no lock the interrupted code
holds (invariants 7 & 8; reuse `lib/` `MpscQueue` + `arrayvec`, use
`debug_assert!`). Symbolize offline. Sample at ~1 kHz, or add a dedicated
higher-frequency profiling timer. Fold host-side into a flamegraph and/or the
Perfetto sample track.

### Phase 3 — unified kernel↔guest stacks (the differentiator)
Extend the unwinder: kernel frames come from DWARF (`from_registers` already does
this via `lib/unwind`); at the host↔guest trampoline, switch to **frame-pointer
walking** through guest frames (safe because `preserve_frame_pointers = true`),
classifying each PC with `lookup_code` (guest) or the backtrace symbolizer
(kernel). Result: single mixed stacks like
`guest_fn → host import → kernel syscall`. This is Wasmtime's approach; both
halves already exist in-tree.

### Phase 4 — whole-system benchmark harness + Perfetto export (medium)
- **Harness:** a `just bench-system` recipe reusing the `just selftests` /
  `build/qemu.bzl` plumbing that boots a `//sys:...-bench` image running a fixed
  wasm workload and prints machine-readable timings (JSON over serial) for CI
  regression tracking. Prefer **QEMU `icount`** for deterministic
  instruction-count benchmarks — wall-clock under QEMU is too noisy to catch
  small regressions; instruction/cycle counts are reproducible. Pin one hart and
  disable KASLR for run-to-run comparability.
- **Export:** converge span events + samples into the one per-CPU ring with the
  common header, drain over serial → a small host converter → **Perfetto**
  (protobuf track-events for duration slices, sample track for callstacks). This
  is the same pipeline Fuchsia and Chrome use and gives the best unified
  kernel+wasm view.

## Open questions / hazards

- **Multi-hart time skew.** Per-hart `time` CSRs can disagree
  (`clock.rs:53` already flags this; issue
  [#490](https://github.com/JonasKruckenberg/k23/issues/490)). Merging per-CPU
  tracks needs a single reference hart or per-CPU offset correction.
- **DWARF unwind from arbitrary PCs.** A timer can land mid-prologue where CFI
  isn't established yet; sampling profilers tolerate this by dropping bad
  samples. Frame-pointer walking is more robust but only for FP-preserving code.
- **Observer effect.** Keep aggregation in-kernel and everything feature-gated +
  runtime-armed so the default build is untouched.
- **Ring buffer correctness.** Producer/consumer ordering needs real
  `Acquire`/`Release`; reuse `lib/` primitives and **loom-test** the ring
  (matches the project's testing ethos — see `contributing/adding-tests.md`).
