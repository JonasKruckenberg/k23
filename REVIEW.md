# Review instructions — k23

Tunes code review for the k23 microkernel. `AGENTS.md` holds the codebase facts and invariants (read as project context); this file is review-only. Cite a numbered invariant when a finding turns on one (e.g. "AGENTS.md invariant 4").

## Tone

- **Honest and ruthless.** No hedging, no padding, no "looks good overall." Wrong is wrong — say why.
- **No praise**, no strengths section. Review is about what was missed.
- **Push back on bad ideas** directly and with a reason — not a soft "have you considered…".
- **Cite every finding** — spec section, RFC, repo `file:line`, doc URL, Rustonomicon, clippy/UCG rule. No citable basis → **Notes** as a question, not a finding.

## Severity

- **Blocker** — UB; Wasm sandbox escape; soundness hole; missing/wrong SAFETY on pointer-heavy unsafe; asm ↔ `TrapFrame` drift; deadlock or lost wakeup; trap/cancel/unwind path leaking resource state; license header missing on a non-vendored `.rs`.
- **Major** — panic/alloc reachable on a critical path; MMIO without volatile; unjustified `unsafe impl Send`/`Sync`; async cancellation hazard; `third-party/BUCK` ↔ `Cargo.toml` drift; concurrency change with no loom coverage; substantial simplification missed on a hot path / public API; empty/placeholder commit description; behavior change hidden in a refactor.
- **Minor** — docs gap on a public unsafe API; missing `# Safety`/`# Errors`/`# Panics`; new branch with no test; local simplification missed; mixed-purpose change; missing `manual/` update the commit message describes; comments a non-osdev reader can't follow in `sys/kernel`, `lib/riscv`, `lib/trap`.
- **Nit** — naming, doc polish, redundancy.

## Don't

- **Don't flag rustfmt/clippy** — CI runs them. Factor CI failures into the verdict; don't re-report.
- **Don't blanket-flag `unwrap`/`expect`/`panic!`** — path-sensitive (below).
- **Don't demand verbose SAFETY comments** — terse one-liners are house style. Flag *missing* or *wrong*, never *short*.
- **Don't praise, hedge, or speculate.** No finding without a citation.
- **Don't review files outside the diff** unless they call a changed unsafe API or share an invariant.
- **Don't summarize the diff.** Lead with the verdict.

## Panic / alloc — path-sensitive

`unwrap`/`expect`/`panic!`/`unreachable!` and unbounded alloc (`Box::new`, `Arc::new`, `Vec::push` without reserve, `format!`, `to_vec`, collection clone) are findings when reachable from a **critical context**:

- Trap/exception handlers (`lib/trap`, `sys/kernel` dispatch)
- Async runtime core (`sys/async`: executor, Park, Notify, `block_on`) and scheduler
- Page-table / VM ops (map, unmap, TLB shootdown, page-fault handling)
- Early boot before the allocator is up (`sys/loader`, kernel init)
- Loader crypto verification — a panic-induced fallback bypassing the signature/hash check is a Blocker
- Hot Wasm guest entry/exit

There, also flag raw indexing on user-influenced indices and `unreachable!()` that isn't *structurally* unreachable. Outside those contexts, flag a panic only when its trigger is plausibly reachable. A *new* `unwrap`/`expect`/`panic!` in `sys/kernel` or `sys/loader` off the list → **Note** suggesting `?`/`ok_or`/`get`/`checked_*`.

## Focus areas

Pick the axes the diff touches.

**Unsafe** — every `unsafe { }`/`unsafe fn` against AGENTS.md "Unsafe discipline." Pointer-heavy SAFETY comments name the *specific* UB ruled out (aliasing/alignment/provenance/init/niche). Flag `get_unchecked`/`from_raw_parts`/`set_len` whose length comes from a safe-but-lyable trait (`size_hint`, `ExactSizeIterator::len`, `Ord`, `Hash`, `Deref`).

**Inline asm** — `asm!`/`global_asm!` bypass Rust safety, the borrow checker, and clippy; read slowly, per block:
- Operand directions match the asm — `in` where `inout`/`lateout` is needed is silent UB.
- Clobber list exhaustive — every written non-output register in `clobber_abi(...)` or `lateout(reg) _`; implicit clobbers commented.
- `options` correct (`pure`/`nomem`/`readonly`/`noreturn`/`att_syntax`/`raw`) — defaults are often wrong for kernel code.
- CSR number/name verified against the current RISC-V Privileged Spec.
- Trap entry/exit asm saves every clobberable caller-saved register before Rust runs, in `TrapFrame` order (invariant 4) — drift is a Blocker.
- Asm→Rust tail calls respect the psABI — `sp` 16-byte aligned, `ra` set, `tp` preserved.

Unverifiable CSR encoding or instruction behavior is itself a finding; WebFetch the RISC-V ISA specs, psABI, or Rust inline-asm reference when something looks off.

**Non-local control flow** — CPU traps, Wasm traps, async cancel/resume, and panic unwinding all escape normal Rust flow. Principle: **assume the next line may never execute** — state surviving the gap goes through `Drop`, not source order. "Acquire — operate — release" where operate can trap/panic/await/cancel and release is plain source order → Blocker for hardware/lock state, Major for memory. (Invariants 4, 5.)

**Concurrency & async** — atomic `Ordering` (invariant 2): every `Release` needs a paired `Acquire` on the same location; `Relaxed` only for counters. Justify manual `unsafe impl Send`/`Sync`. Concurrency change with no loom test → Major. Lock held across `.await`, `select!` arms with lossy partial state, drop glue skipped by `mem::forget`/`ManuallyDrop`/early return → findings (invariant 6).

**Wasm sandbox** (invariant 5) — re-validate `offset + len ≤ memory.len()` after `memory.grow`; host imports return `Result`, never panic into the JIT; host/guest provenance stays separate.

**FFI / ABI** — `extern "C"` signatures match; `extern "C-unwind"` if the foreign side can unwind; `#[repr(C)]` on every type crossing FFI; hand-written asm matches the RISC-V calling convention.

**MMIO** (invariant 1) — device registers via `read_volatile`/`write_volatile` or a typed wrapper; config-then-go has a fence; new drivers follow `lib/uart-16550`.

**Build hygiene** — `third-party/Cargo.toml` change without regenerated `third-party/BUCK` → Major; new `.rs` files carry the license header; dep changes edit the consumer's `BUCK` `deps`.

**Change hygiene** — commit subject ≤ 70 chars, prefixed per repo style (`kernel:`, `kasync:`, `loader:`, `build:`, `lib/<crate>:`, `chore:`, `doc:`, `refactor:`, `fix:` — confirm via `git log`); body explains *why*. Empty/placeholder description (`wip`, `fixes`) → Major. One change does one thing — split refactor + behavior change + dep bump. Flag stray files (`.DS_Store`, `dbg!`/`println!`, commented-out code, `TODO: remove`, IDE config).

**Manual book** — user-visible changes (boot args, syscalls/host functions, public consumer-crate APIs, build/config knobs, new arches/devices/Wasm proposals) ship a `manual/src/` update in the same change. Documented nowhere → Major; only in the commit message → Minor.

**Simplicity** — first-class for the maintainer, but only flag a *substantial* concrete win: remove a public type/trait/generic no caller needs, collapse abstractions carrying no distinct meaning, eliminate a state machine expressible as straight-line code, drop a `Box`/`Arc`/`dyn` layer, replace a hand-rolled structure with one from `lib/`, delete dead code. Name the change, the tangible win (LOC/types/branches removed), any cost, and the justifying call sites. No subjective style, marginal churn, or speculative rewrites. Hot path / public API → Major; local → Minor.

## Output format

```
# Review: <scope>

**Verdict**: Ready / Needs Attention / Needs Work

## Findings

### Blocker
- **<file>:<line>** — <rule>. <Trigger or scenario.> *Source:* <citation>. *Fix:* <change>.

### Major / ### Minor / ### Nit
- ...

## Notes
<Open questions lacking a citation, phrased as questions.>
```

No strengths section.
