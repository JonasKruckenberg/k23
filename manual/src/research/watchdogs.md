# Watchdogs

Research notes on watchdog mechanisms — what they are, how the three target
architectures expose them, what production kernels actually use them for, and
which of it k23 needs.

**Summary of the conclusion:** the mechanism k23 most urgently needs is *not* a
hardware watchdog. It is guest preemption — the ability to stop a WebAssembly
program that never returns. The scaffolding for that already exists in the tree
but is inert. A hardware watchdog is worth building later, as a small `lib/`
crate, and mostly matters on real hardware rather than under QEMU.

## 1. Four things called "watchdog"

The word covers at least four distinct mechanisms. They differ in *what fails*,
*who notices*, and *what is trusted to still be working at the moment of
detection* — the last one is the important axis, because it determines whether a
given mechanism can detect a given failure at all.

| # | Mechanism | Detects | Detector lives in | Trusts |
|---|---|---|---|---|
| 1 | **Hardware watchdog timer** | Anything that stops software from running, including total CPU wedge | Silicon, outside the CPU | Nothing but its own clock |
| 2 | **Hard-lockup detector** | CPU spinning with interrupts disabled | NMI handler | NMI delivery works |
| 3 | **Soft-lockup / liveness detector** | Code looping without yielding; stalled scheduler | Timer interrupt handler | Interrupts still arrive |
| 4 | **Task-liveness watchdog** | A task that stopped making progress, but the system is otherwise fine | A normal task or thread | Whole kernel is healthy |

The escalation is deliberate: each layer detects failures the layer below it
cannot, and each depends on strictly less of the system still working. A
hardware watchdog is the only one that survives a completely dead CPU, which is
why it is the only one that can be trusted to *recover* rather than merely
*report*.

A related distinction, which matters for design: a watchdog can **report** (log,
dump, panic) or **recover** (reset, restart a component). Reporting needs the
system to be healthy enough to write output. Recovery does not. Two-stage
watchdogs exist precisely to get both — a first stage that reports while the
system can still talk, and a second stage that resets when it can't.

## 2. Platform interfaces

### Arm — the cleanest story

Arm standardized this in the Server Base System Architecture. The **SBSA Generic
Watchdog** is architecturally specified, so one driver works across vendors.

It presents *two* MMIO frames, which is a genuinely good design idea worth
stealing:

- **Refresh frame** — contains only `WRR` (offset `0x000`). Writing anything to
  it reloads the counter.
- **Control frame** — `WCS` (`0x000`, enable + status), `WOR` (`0x008`, the
  offset/reload value), `WCV_LO`/`WCV_HI` (`0x010`/`0x014`, the 64-bit compare
  value).

Splitting refresh from control means the refresh frame can be mapped into a
lower-privilege or otherwise restricted context that is allowed to *pet* the
watchdog but not to *disarm* it. That property is the whole point, and it is
lost in designs with a single control register.

Operation is two-stage. When `WCV` is reached the first time, **WS0** fires as
an interrupt and `WCV` is reloaded from `WOR`; if the second period also
elapses, **WS1** fires and the system resets. WS0 is the pretimeout — the
window in which the kernel can still log, dump, and panic. Discovery is via the
ACPI GTDT or the device tree.

Deeply-embedded Arm parts instead usually ship the older Arm SP805, which is
per-SoC rather than architectural.

**QEMU:** `-device sbsa-gwdt`, on the `virt` machine, described to the guest via
GTDT and FDT.

### x86 — layered accretion

Three separate mechanisms, reflecting three eras:

- **iTCO** — the Intel TCO timer in the ICH/PCH. Registers live in an ACPI/PMC
  I/O block; the machine reboots on the timer's *second* expiration, which is
  the same two-stage idea in a different dress. Driver: `iTCO_wdt`.
- **ACPI WDAT** (Watchdog Action Table) — an abstraction layer. Instead of a
  register map, firmware ships a table of *actions* ("to pet, write value V to
  address A"), and the OS interprets it. Linux prefers `wdat_wdt` over
  `iTCO_wdt` where both are present, because the firmware description is the
  OEM-validated one. WDAT supersedes the earlier, deprecated WDRT.
- **The perf/NMI watchdog** — not a device at all. It programs a performance
  counter to overflow into an NMI, and the NMI handler checks whether the timer
  interrupt has been servicing this CPU. This is mechanism #2 from the table
  above, and it is a pure-software use of an interrupt-delivery feature.

Server BMCs add their own: HPE's iLO, for example, delivers a pretimeout NMI
about nine seconds before it resets the box, specifically so the kernel can
panic into a kdump.

### RISC-V — there is no standard, and that's the finding

This is the part worth being blunt about, because it directly constrains what
k23 can portably do.

**There is no ratified RISC-V watchdog specification.** A standalone one was
drafted — a memory-mapped device with a single 32-bit `WDCSR` register:

| Field | Bits | Meaning |
|---|---|---|
| `WDEN` | 0 | Enable |
| `S1WTO` | 2 | Stage-1 timeout occurred |
| `S2WTO` | 3 | Stage-2 timeout occurred |
| `WTOCNT` | 13:4 | 10-bit timeout count, in watchdog ticks |

A tick is defined as a selected bit of `MTIME` going 0→1, so the period is
`WTOCNT × MTIME_resolution × 2^(bit+1)` seconds; the spec recommends a tick
between 0.1 s and 1 s. Petting is a single write with `WDEN=1`, both `SxWTO`
bits clear, and `WTOCNT` reloaded — no magic key sequence, and notably no
privilege split like Arm's refresh frame. Two-stage semantics match Arm's:
stage 1 raises an interrupt, stage 2 raises a second, higher-privilege one.

That document reached **draft 0.5 and its repository was archived in January
2023**. It was never ratified. It also never specified a discovery mechanism —
it explicitly leaves the `MTIME` bit position and resolution to "platform-specific
means." The current RISC-V server platform requirements document does not
mandate a watchdog either.

The practical consequence: **on RISC-V, watchdogs are per-SoC device-tree
drivers.** There is nothing to write a portable driver against. There is also
no watchdog extension in SBI — SBI gives you `TIME` (`set_timer`), `IPI`,
`SRST` (system reset, which is the useful *action* half), `HSM`, and friends,
but nothing that arms an independent timer.

For the NMI half of the picture — mechanism #2, detecting a hart spinning with
interrupts off — RISC-V has two candidates, and k23 can use neither today:

- **`Smrnmi`** — resumable NMI. Adds `mnepc`/`mncause`/`mnstatus`/`mnscratch`
  and the `MNRET` instruction. RNMIs outrank every other trap and are not
  masked by `mstatus.MIE`. But it is an **M-mode** facility, and k23 runs in
  S-mode under SBI; a kernel cannot install an RNMI handler for itself.
- **SBI SSE** (Supervisor Software Events) — lets firmware inject NMI-like
  events into S-mode that can interrupt the kernel even inside a trap handler,
  originally motivated by RAS. This is the *right* mechanism for a RISC-V
  hard-lockup detector, but it requires SBI implementation support and a new
  extension in `lib/riscv/src/sbi/` (which today has `base`, `dbcn`, `hsm`,
  `ipi`, `rfence`, `time`).

**QEMU:** the RISC-V `virt` machine has **no** built-in watchdog device — unlike
Arm's `virt`, which has `sbsa-gwdt`. It does have a PCIe host bridge, so the
generic `i6300esb` PCI watchdog is the plausible route. *This is untested;* it
should be verified before any design depends on it.

### Comparison

| | Arm | x86 | RISC-V |
|---|---|---|---|
| Architectural spec | Yes (SBSA GWDT) | No (de-facto iTCO) | **No** |
| Firmware abstraction | GTDT | WDAT (preferred) | — |
| Two-stage | Yes (WS0/WS1) | Yes (2nd expiry) | In the unratified draft |
| Privilege-split refresh | **Yes** | No | No |
| NMI path for hard-lockup | Yes | Yes (perf NMI) | `Smrnmi` (M-mode) / SBI SSE |
| QEMU `virt` support | `sbsa-gwdt` | — | **none built in** |

## 3. What production kernels use them for

Linux is the reference point, and it runs all four mechanisms:

- **Hard-lockup detector** (`nmi_watchdog`) — a perf NMI event checks whether
  the timer interrupt has fired on this CPU. Catches a CPU looping with
  interrupts disabled, which nothing else can see.
- **Soft-lockup detector** — an hrtimer callback compares against a timestamp
  updated by a per-CPU stop-scheduler thread. If the timestamp is stale for
  `2 × watchdog_thresh` (20 s by default), it dumps and optionally panics.
  Catches kernel code that loops without scheduling.
- **Hung-task detector** (`khungtaskd`) — walks the task list every 120 s
  looking for tasks stuck in `TASK_UNINTERRUPTIBLE` that haven't been scheduled
  in that window. Catches deadlocks and lost wakeups, not CPU hogging.
- **RCU CPU stall detector** — reports CPUs that fail to pass through a
  quiescent state during a grace period. Only fires while a grace period is in
  flight.
- **Hardware watchdog** via `/dev/watchdog` — a character device: open to arm,
  write (or `WDIOC_KEEPALIVE`) to pet, close cleanly to disarm. Pretimeout
  governors turn the first stage into a panic so kdump can capture a dump
  before hardware resets the machine.
- **Userspace liveness** — systemd's `WatchdogSec` has services ping the
  supervisor, and systemd in turn pets `/dev/watchdog`, chaining application
  liveness to hardware recovery. Immutable/appliance distributions lean on this
  heavily.

The design pattern underneath all of it: **a chain of petting, from the highest
layer down to hardware.** Each link proves the layer above is alive. The
hardware timer at the bottom is the only unfalsifiable link.

The embedded/RTOS world takes a different tack that is closer to k23's spirit.
Oxide's Hubris, for instance, deliberately does *not* restart faulted tasks in
the kernel; a designated **supervisor task** is notified and decides policy,
and the documentation is explicit that leaving a faulted task blocked and
"expecting a watchdog timer to handle the problem if it matters" is a legitimate
architecture. Detection and policy are separated, and policy lives outside the
kernel.

### Which of these does k23 need?

| Linux mechanism | k23 analogue | Verdict |
|---|---|---|
| Hard-lockup (NMI) | none possible today | **Defer** — needs SBI SSE |
| Soft-lockup | executor liveness | **Yes, cheap** |
| Hung task | stalled `kasync` task | **Later** — needs task-level accounting |
| RCU stall | n/a — no RCU | **No** |
| Hardware watchdog | none | **Yes, but later** — matters on real hardware |
| Userspace liveness | guest liveness | **Yes — and this is the urgent one** |

## 4. Where k23 stands today

### The hole that matters: guests cannot be preempted

`sys/kernel/src/wasm/` already carries the *entire data layout* for Wasmtime-style
epoch interruption:

- `wasm/engine.rs:35` — `epoch_counter: AtomicU64` on `EngineInner`, exposed at
  `engine.rs:99`.
- `wasm/vm/vmcontext.rs:871` — `epoch_deadline: UnsafeCell<u64>` in the
  `VMContext`, alongside `fuel_consumed` at `:865`.
- `wasm/vm/vmshape.rs:139` — `vmctx_epoch_ptr()`, the compiled-code offset.
- `wasm/vm/instance.rs:737` — the instance actually writes the engine's counter
  pointer into the vmctx on activation.

And yet:

- **Nothing ever increments `epoch_counter`.** The only references are the
  constructors initialising it to `0` and the accessor.
- **Cranelift emits no epoch checks.** There is not a single mention of `epoch`
  or `fuel` anywhere under `wasm/cranelift/`.

So the pointer is plumbed to a counter that never changes, and no compiled code
would look at it anyway. **A guest with an infinite loop wedges the kernel
permanently.** The executor is cooperative (`sys/async/src/executor.rs:262`
ticks, then parks in `sleepers.wait()`), currently single-worker
(`main.rs:186`, `Executor::with_capacity(1)`, with the comment "single-CPU at
handoff is the current contract"), and parking is `wfi`
(`arch/riscv64/block_on.rs`). Nothing takes the CPU back from a running guest.

This is a **sandbox-boundary problem**, not just a liveness one. Critical
invariant 5 says the WASM sandbox must hold; a guest that can deny the CPU to
the whole system indefinitely is a sandbox escape in the availability
dimension. It is the single highest-value item in this document.

### What does exist

k23 is not defenceless — it has good *fault* recovery, just no *hang* recovery:

- Kernel traps unwind rather than halting
  (`arch/riscv64/trap_handler.rs:356`, `handle_kernel_exception` →
  `panic_unwind::begin_unwind`), with a root `catch_unwind` in `_start`
  (`main.rs:82`).
- Recursive traps are detected via the per-CPU `IN_TRAP` flag
  (`trap_handler.rs:284`) and given their own path.
- Guest traps are caught and turned into guest-visible traps
  (`wasm/trap_handler.rs`, via `end_wasm_trap`).
- A timer wheel exists and is already driven from the trap handler
  (`trap_handler.rs:300–307`: `SupervisorTimer` → `timer.try_turn()` →
  `executor.wake_one()`), backed by `sbi::time::set_timer`
  (`arch/riscv64/device/clock.rs:33`) at 1 ms granularity (`main.rs:187`).
- A per-CPU counter facility exists (`metrics.rs`) that a detector could report
  through for free.

The timer trap arm is the key asset. **It is the only place in the system that
runs asynchronously with respect to whatever the CPU was doing** — which makes
it the natural home for every software detector below.

## 5. Proposal

Four tiers, in strict value order. Tiers 0 and 1 need no new hardware support
and are worth doing regardless of what any platform provides.

### Tier 0 — Epoch-based guest preemption *(the actual priority)*

Finish what the tree already started. Epoch interruption is the right choice
over fuel metering: the codegen is a load of a global counter plus a
compare-and-branch at loop backedges and function prologues, whereas fuel
instruments every operation and measures roughly 2–3× slower on
control-flow-heavy workloads. Fuel's advantage is determinism, which k23 does
not currently need. (Fuel can come later for the deterministic-execution use
case; the `fuel_consumed` field is already there.)

Three pieces:

1. **Bump the counter.** In the `SupervisorTimer` arm of
   `arch/riscv64/trap_handler.rs`, or from a periodic `kasync` task. The
   handler is on a critical path, so this must be exactly one
   `fetch_add(1, Relaxed)` — no locks, no allocation, per invariants 7 and 8.
   `Relaxed` is correct here and should be justified explicitly in a comment:
   the counter is a monotonic tick with no happens-before relationship to
   guest memory (invariant 2), and the guest-side check is a plain load.
2. **Emit the check.** In `wasm/cranelift/`, at function prologues and loop
   backedges: load `epoch_ptr`, compare against the vmctx `epoch_deadline`,
   and branch to a builtin on overrun. Wasmtime's approach caches the deadline
   in a register within a function body and only reloads after calls, which
   keeps the common case to a compare-and-branch.
3. **Handle the overrun.** The builtin either traps the guest (turning it into
   an ordinary guest trap through the existing `wasm/trap.rs` path) or yields
   to the executor and extends the deadline. Both are useful: yield for
   timeslicing, trap for a runaway. Note that the host callback must **not**
   panic into JIT frames — invariant 5.

Testing: a `.wast` fixture with an infinite loop, asserted to trap rather than
hang, run under the existing `just selftests` harness.

This turns "guest hangs the kernel forever" into "guest gets descheduled or
killed," and it is the difference between k23 being able to run untrusted code
and not.

### Tier 1 — Executor liveness detector (soft-lockup analogue)

Cheap, self-contained, and immediately useful for debugging.

A per-CPU heartbeat: `Worker::run`'s tick loop
(`sys/async/src/executor.rs:262`) stores the current tick count or timestamp
into a per-CPU `AtomicU64`. The timer trap arm compares it against the previous
observation; if it is unchanged for N ticks *and* the worker is not legitimately
parked in `wfi`, log a warning and a backtrace — `sys/backtrace/` can already
produce one from a trap frame, as `handle_kernel_exception` demonstrates.

Two design points worth getting right:

- **Distinguishing "stalled" from "idle" is the whole problem.** A parked worker
  has an unchanging heartbeat and is perfectly healthy. The park path in
  `block_on.rs` must set an "intentionally parked" flag that the detector
  respects, or the heartbeat must be sampled only when the run queue is
  non-empty.
- **The detector must not panic.** It runs in a trap handler — invariant 8. It
  reports; it does not act. Threshold via `bootargs` (`sys/kernel/src/bootargs.rs`),
  mirroring Linux's `watchdog_thresh`.

With `-smp cpus=8` in the QEMU config but one CPU used at handoff, this becomes
substantially more valuable as SMP lands — that is exactly when lost wakeups and
stuck workers start to appear.

### Tier 2 — A hardware watchdog abstraction

This belongs in `lib/`, per the "could this be a `lib/` crate?" test — it is
kernel-agnostic and useful to any `no_std` supervisor. Call it `lib/watchdog`.

```rust
/// A hardware watchdog timer.
pub trait Watchdog {
    /// Arm the watchdog with the given timeout.
    fn arm(&mut self, timeout: Duration) -> Result<(), Error>;
    /// Reload the counter. Must be called more often than `timeout`.
    fn pet(&mut self);
    /// Disarm, if the device supports it.
    fn disarm(&mut self) -> Result<(), Error>;
    /// Whether the last boot was caused by this watchdog firing.
    fn fired_last_boot(&self) -> bool;
}
```

Notes on the shape:

- `fired_last_boot()` earns its place — distinguishing a watchdog reset from a
  clean boot is most of the diagnostic value, and both the Arm and RISC-V
  designs expose latched status bits for it.
- Register access is MMIO and must go through `read_volatile`/`write_volatile`
  with a fence on the arm sequence — invariant 1, exactly as `lib/uart-16550/`
  does it.
- Petting is driven by a low-priority `kasync` task, so that petting *proves the
  executor is alive*. Petting from the timer interrupt would be a lie: it would
  keep the machine alive while the scheduler is dead. This is the chain-of-petting
  principle from §3, and getting it backwards is the classic watchdog bug.
- Two-stage where available: wire stage 1 to a panic-and-report path (so
  `sys/backtrace/` can dump before the reset), stage 2 to the hardware reset.
  This mirrors Linux's pretimeout-governor design.
- The action on RISC-V should route through `sbi::srst` — a new module in
  `lib/riscv/src/sbi/`, which does not have one yet.

Discovery goes through the existing `device_tree.rs`. Realistically, the first
concrete backend is per-SoC; there is no architectural RISC-V device to target.
**Before committing to this tier, verify whether `-device i6300esb` attaches to
QEMU's RISC-V `virt` PCIe bridge** — without an emulated device there is nothing
to test against in CI, which materially weakens the case for building it now.

### Tier 3 — Hard-lockup detection *(defer)*

Requires NMI-like delivery into S-mode. On RISC-V that means SBI SSE, which
needs both an SBI implementation that supports it and a new extension module in
`lib/riscv/src/sbi/`. `Smrnmi` is M-mode-only and therefore unavailable to k23
under SBI. Not worth pursuing until the SSE ecosystem is real — and note that
Tier 0 removes the most likely *cause* of a wedged CPU anyway.

## 6. Recommendation

**Do Tier 0 now.** It is the one item that closes a real sandbox-boundary gap
(invariant 5), the data layout is already in the tree, and the design question
— epoch versus fuel — has a well-evidenced answer. Everything else in this
document is diagnostics; this is correctness.

**Do Tier 1 next, opportunistically.** It is perhaps a hundred lines, it makes
the SMP work that is coming much more debuggable, and it costs one atomic store
per scheduler tick.

**Hold Tier 2 until there is hardware or a verified emulated device.** The
abstraction is easy to design — and the design above is worth keeping — but a
watchdog with no backend to test against is speculative. The one piece worth
doing early regardless is `sbi::srst`, since a reset path is independently
useful.

**Skip Tier 3 for now.**

The thing to resist is building a hardware-watchdog subsystem *first* because it
is the most recognisably "watchdog-shaped" work. It would not have caught the
failure k23 actually has today: an infinite loop in a guest, which a hardware
watchdog would answer by resetting the whole machine, when the correct answer is
to trap one guest function and keep running.

---

## Sources

- [Softlockup and hardlockup detectors — Linux kernel docs](https://docs.kernel.org/admin-guide/lockup-watchdogs.html)
- [Using RCU's CPU Stall Detector — Linux kernel docs](https://docs.kernel.org/RCU/stallwarn.html)
- [The Linux Watchdog driver API](https://www.kernel.org/doc/html/latest/watchdog/watchdog-api.html)
- [The Linux WatchDog Timer Driver Core kernel API](https://docs.kernel.org/6.2/watchdog/watchdog-kernel-api.html)
- [HPE iLO NMI Watchdog Driver](https://github.com/torvalds/linux/blob/master/Documentation/watchdog/hpwdt.rst)
- [linux/kernel/hung_task.c](https://github.com/torvalds/linux/blob/master/kernel/hung_task.c)
- [ACPI / watchdog: Add support for WDAT — LWN](https://lwn.net/Articles/701235/)
- [CONFIG_ITCO_WDT: Intel TCO Timer/Watchdog](https://cateee.net/lkddb/web-lkddb/ITCO_WDT.html)
- [Watchdog Descriptor Table (WDRT) — UEFI.org](https://uefi.org/sites/default/files/resources/Watchdog%20Descriptor%20Table.pdf)
- [Introduce ARM SBSA watchdog driver — LKML](https://lkml.iu.edu/hypermail/linux/kernel/1602.2/00671.html)
- [riscv-watchdog specification (archived)](https://github.com/riscvarchive/riscv-watchdog/blob/main/riscv-watchdog.adoc)
- [RISC-V SBI Supervisor Software Events (SSE)](https://github.com/riscv-non-isa/riscv-sbi-doc/blob/master/src/ext-sse.adoc)
- [riscv: add support for SBI Supervisor Software Events — LWN](https://lwn.net/Articles/948947/)
- ["Smrnmi" Extension for Resumable Non-Maskable Interrupts](https://docs.riscv.org/reference/isa/v20260120/priv/rnmi.html)
- [QEMU 'virt' generic virtual platform (Arm)](https://qemu-project.gitlab.io/qemu/system/arm/virt.html)
- [QEMU 'virt' Generic Virtual Platform (RISC-V)](https://www.qemu.org/docs/master/system/riscv/virt.html)
- [qemu/hw/watchdog/wdt_i6300esb.c](https://github.com/qemu/qemu/blob/master/hw/watchdog/wdt_i6300esb.c)
- [Wasmtime: epoch-based interruption (PR #3699)](https://github.com/bytecodealliance/wasmtime/pull/3699)
- [Wasmtime `Config` — fuel and epoch interruption](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html)
- [Wasmtime: Interrupting Execution](https://docs.wasmtime.dev/examples-interrupting-wasm.html)
- [Hubris Reference — supervisor and task faults](https://hubris.oxide.computer/reference/)
