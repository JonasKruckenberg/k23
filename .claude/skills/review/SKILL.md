---
name: review
description: Code review for the k23 microkernel — unsafe soundness, panic/alloc on critical paths, RISC-V atomic ordering, async cancellation, MMIO volatility, FFI/ABI, Wasm sandbox. Runs `just preflight` in the background and folds the result in. Trigger when the user asks to review a change, branch, or diff in this repo.
argument-hint: "[optional: path, crate, or git revision range; defaults to working copy + branch vs main]"
allowed-tools: Read, Grep, Glob, Bash, Agent, WebFetch, WebSearch
---

# Review — k23

Review the current branch (working copy + commits since `main`) for issues that matter in *this* codebase. `AGENTS.md` holds the codebase facts and eight numbered invariants — **cite the invariant number when a finding turns on one** (e.g. "AGENTS.md invariant 4 — trap frame layout").

## Tone

- **Honest and ruthless** — no hedging, no praise, no "looks good overall". Wrong is wrong; say so with the citation. Junior contributors deserve a clear "this is wrong because…", not a soft "have you considered…".
- **Cite every finding** — AGENTS.md invariant, spec section, RFC, repo `file:line`, doc URL, Rustonomicon, clippy/UCG rule. No citable basis → **Notes** as a question, not a Finding.
- **Don't summarize the diff.** Lead with the verdict.

## Severity

- **Blocker** — UB; Wasm sandbox escape; soundness hole; missing/wrong SAFETY on pointer-heavy unsafe; asm ↔ `TrapFrame` drift; deadlock or lost wakeup; trap/cancel/unwind path leaking resource state; license header missing on a non-vendored `.rs`.
- **Major** — panic/alloc reachable on a critical path; MMIO without volatile; unjustified `unsafe impl Send`/`Sync`; async cancellation hazard; `third-party/BUCK` ↔ `Cargo.toml` drift; concurrency change with no loom coverage; substantial simplification missed on a hot path / public API; empty/placeholder commit description; behavior change hidden in a refactor.
- **Minor** — docs gap on a public unsafe API; missing `# Safety`/`# Errors`/`# Panics`; new branch with no test; local simplification missed; mixed-purpose change; missing `manual/` update the commit message describes; non-osdev-friendly comments in `sys/kernel`, `lib/riscv`, `lib/trap`.
- **Nit** — naming, doc polish, redundancy.

## Effort

User may pass **light** / **medium** / **thorough** (default: medium). Pass the level verbatim into each subagent.

- **light** — orchestrator only, no subagents, highest-risk axes only. Docs/comment fixes, one-line bugfixes.
- **medium** — 2–4 specialist subagents on axes the diff touches.
- **thorough** — all relevant subagents, deep pass, WebFetch specs as needed, Grep callers for every changed unsafe API or trait impl.

## Workflow

1. **Run inside the nix devshell.**
2. **Capture the diff** — `git diff --stat main` then `git diff main`. Honor the argument if given. Repo is jj+git colocated; CI runs git, so use git here.
3. **Kick off preflight in the background** — `just preflight` (run_in_background=true). Runs clippy + check-fmt + typos + unittests + miri + loom + selftests + buck2-audit + cargo-deny + license-header. Don't block on it.
4. **Read changed files in full.** Diffs hide invariants. Grep callers when an unsafe API or signature changes.
5. **Fan out specialist subagents** — see below.
6. **Synthesize** — wait for subagents and preflight, fold findings in, dedupe, classify, emit. **Do not auto-fix** — review surfaces findings; the user asks for fixes if they want them.

## Subagent fan-out

For medium+ effort, distribute the review across read-only `general-purpose` subagents, one per axis.

- **Single Agent call, multiple invocations** so they run concurrently. Sequencing defeats the point.
- **Pass each the full diff** plus the file list — don't make them re-derive scope.
- **Pass the effort level** verbatim.
- **Pass the axis** and quote the relevant calibration paragraph from this file — don't make them guess.
- **Pass the output spec**: findings list with Blocker/Major/Minor/Nit severity and a citation per finding. They return findings; they do not fix.
- **Pick 2–5 axes the diff touches.** A docs-only diff doesn't need an inline-asm pass.

**Axis cheatsheet:**

- `unsafe { }` / `unsafe fn` → Unsafe
- `asm!` / `global_asm!` → Inline-asm (always, even one-line)
- atomics, locks, `Send`/`Sync`, `sys/async` → Concurrency + Async
- trap dispatch, Wasm runtime, `.await` + cleanup → Non-local control flow
- MMIO / drivers → MMIO
- `extern "C"` / FFI → FFI/ABI
- `sys/kernel/src/wasm` → Wasm sandbox
- `third-party/Cargo.toml`, BUCK, new `.rs` → Build hygiene
- **Always**: Simplicity, Change hygiene, Documentation.

For deep simplification, also consider a parallel pass with the `simplify` skill.

## Calibrated rules

### Simplicity & elegance — first-class

Flag only **substantial structural wins**:
- Remove a public type/trait/generic no caller benefits from
- Collapse two abstractions carrying no distinct meaning
- Eliminate a state machine expressible as straight-line code
- Drop a `Box`/`Arc`/`Option`/`Result`/`dyn` layer when callers want the wrapped form
- Swap runtime dispatch ↔ monomorphization when it removes branches without harming clarity
- Replace ad-hoc bit manipulation with named consts or `bitflags!`
- Reduce parameter count by extracting a struct or splitting a two-job function
- Delete dead code (unused features, fields, branches, error variants)
- Replace a hand-rolled structure with one in `lib/` (`wavltree`, `sharded-slab`, `range-tree`, `arrayvec`, `spin`)

Each finding names: the concrete change, the tangible win (LOC/types/branches/generics removed), any cost, and the justifying call sites. Hot path / public API → Major; local → Minor.

**No** subjective style, marginal churn, speculative rewrites, or pattern-matching against other codebases.

### Unsafe (AGENTS.md "Unsafe discipline")

- Every `unsafe { }` has a `// SAFETY:` comment. **Terse is house style** — flag missing or wrong, never short.
- Every `unsafe fn` has a `# Safety` doc section.
- Manual `unsafe impl Send`/`Sync` justifies itself against interior state (raw ptrs, `Cell`, non-`Send` fields).
- Pointer-heavy SAFETY names the *specific* UB ruled out (aliasing/alignment/provenance/init/niche). Vague doesn't count.
- Flag `get_unchecked` / `from_raw_parts` / `set_len` whose length comes from a safe-but-lyable trait (`size_hint`, `ExactSizeIterator::len`, `Ord`, `Hash`, `Deref`).
- Inside `unsafe fn`, every unsafe op in an explicit inner `unsafe { }` (Rust 2024 `unsafe_op_in_unsafe_fn`).

### Inline assembly

`asm!` / `global_asm!` bypass Rust safety, the borrow checker, and clippy — read slowly, per block:

- **Operand directions** match what the asm does. `in` where `inout`/`lateout` is needed is silent UB.
- **Clobber list** exhaustive: every written non-output register in `clobber_abi(...)` or `lateout(reg) _`. Implicit clobbers (e.g. `mstatus` after a side-effecting CSR write) need a comment.
- **`options`** (`pure`/`nomem`/`readonly`/`noreturn`/`att_syntax`/`raw`) — defaults are often wrong for kernel code.
- **CSR access** — verify number/name against the current RISC-V Privileged Spec; encoding errors are silent. Cite the section.
- **Memory ordering** — asm with no `mem` access inserts no fence. If the asm *is* a fence (`sfence.vma`), surrounding Rust must not assume reordering protection beyond what the instruction provides.
- **Trap entry/exit asm** (`lib/trap`, `sys/kernel`): every caller-saved register saved before Rust runs, in `TrapFrame` order (invariant 4). Drift → **Blocker**.
- **Asm → Rust tail calls** respect the psABI: `sp` 16-byte aligned, `ra` set, `tp` preserved.

WebFetch when unsure — specs evolve. Cannot verify a CSR encoding / instruction / operand → that's itself a finding. Authoritative sources:
- [RISC-V Privileged Spec](https://riscv.org/specifications/privileged-isa/) — CSRs, traps, fences, SATP, sstatus
- [RISC-V Unprivileged ISA Spec](https://riscv.org/specifications/) — encoding, base ISA, extensions
- [RISC-V psABI](https://github.com/riscv-non-isa/riscv-elf-psabi-doc) — calling convention
- [Rust Reference: Inline Assembly](https://doc.rust-lang.org/reference/inline-assembly.html) — operands, options, clobbers
- [Rust Unstable Book: `asm`](https://doc.rust-lang.org/unstable-book/library-features/asm.html)

### Panic / alloc — path-sensitive (AGENTS.md invariant 8)

`unwrap`/`expect`/`panic!`/`unreachable!` and unbounded alloc (`Box::new`, `Arc::new`, `Vec::push` without reserve, `format!`, `to_vec`, clone) are findings only when reachable from a **critical context**:

- Trap / exception handlers (`lib/trap`, `sys/kernel` dispatch)
- Async runtime core (`sys/async`: executor, Park, Notify, `block_on`) and scheduler
- Pre-allocator-init paths (early `sys/loader`, kernel init)
- Page-table / VM ops (map, unmap, TLB shootdown, page-fault) — unrecoverable
- Loader crypto verification — panic-induced fallback bypassing the signature/hash → **Blocker**
- Hot Wasm guest entry/exit

In those, also flag raw indexing on user-influenced indices and `unreachable!()` that isn't *structurally* unreachable. Outside, flag only when the trigger is plausibly reachable. Prefer `debug_assert!` for invariant checks. A **new** `unwrap`/`expect`/`panic!` in `sys/kernel` or `sys/loader` off the list → **Note** suggesting `?` / `ok_or` / `get` / `checked_*`.

### Concurrency (AGENTS.md invariant 2)

- `Relaxed` only for counters with no happens-before. Synchronization needs Acquire/Release+.
- Every `Release` write needs a paired `Acquire` read on the same location.
- MMIO config → "go" bit needs an explicit fence — RISC-V doesn't order device accesses against normal memory.
- Concurrency change without loom test (`just loom`) → **Major**.

### Async (AGENTS.md invariant 6)

- Lock held across `.await` → finding (cancel drops the future; state is left invalid).
- `select!` arms with lossy partial state (half-read buffer, partial transaction) → finding. Hoist outside the loop.
- Drop glue for hardware cleanup must survive cancel — flag `mem::forget`, `ManuallyDrop`, early returns skipping it.
- `Park` / `Notify` impls: justify every `unsafe impl Send`/`Sync` against the underlying primitive.

### Non-local control flow (invariants 4, 5, 6)

Four control paths in k23 escape normal Rust flow: **CPU traps**, **Wasm traps**, **async cancellation**, **panic unwinding** (where `panic = "unwind"`). Principle: **assume the next line may never execute** — state surviving the gap goes through `Drop`, not source order.

- **CPU traps** (`lib/trap`, `sys/kernel` dispatch): asm save sequence must match `TrapFrame` (invariant 4); handler that allocates, locks something held by interrupted code, or panics → **Blocker**.
- **Wasm traps**: host imports must hold no locks/allocations/non-`Drop` state across calls into JIT (invariant 5). Cite Wasmtime for the comparable case.
- **Panic unwinding**: between two operations, the second may be skipped. Cleanup goes in RAII. If the crate is `panic = "abort"` this is moot — cite the `Cargo.toml`/BUCK.

"Acquire — operate — release" where the operate step can trap/panic/await/cancel and release is plain source order → **Blocker** for hardware/lock state, **Major** for memory.

### MMIO (AGENTS.md invariant 1)

- Device registers via `read_volatile`/`write_volatile` or a typed wrapper. Plain field access through `&mut` to MMIO is UB.
- Config-then-go sequences need an explicit fence.
- New drivers follow `lib/uart-16550`.

### FFI / ABI

- `extern "C"` signatures match across the boundary.
- Foreign side can unwind → `extern "C-unwind"`; otherwise unwinding is UB.
- `#[repr(C)]` on every type crossing FFI; field order/padding/enum repr are part of the contract.
- Handwritten asm in `lib/riscv` and `lib/trap`: register save/restore matches the riscv calling convention.

### Wasm sandbox (AGENTS.md invariant 5)

- Re-validate `offset + len ≤ memory.len()` after any potential `memory.grow`.
- Host imports return `Result`, never panic into the JIT.
- Host and guest pointer provenance stay separate.

### Build hygiene

- `third-party/Cargo.toml` change without regenerating `third-party/BUCK` via `just buckify` (reindeer) → **Major**.
- New `.rs` carries the canonical license header (`Copyright 2023-Present`), enforced by `//build/license-header-linter` (`just check-license-headers`; `just fix-license-headers` to add it). Vendored exempts: `lib/range-tree`, `lib/sharded-slab`, `lib/wast`.
- Adding/changing internal deps requires editing the consumer's `BUCK` `deps` — `just check` catches it.
- New crates follow `manual/src/contributing/adding-a-crate.md`.

### Documentation & comments

k23's audience is strong Rust engineers, **not** osdev/riscv/compiler experts. Comments are a teaching surface.

**Public APIs**: every `pub` item has a doc comment with *what* and *when to use it*. `# Errors` on every public `fn` returning `Result`; `# Panics` on every public `fn` that can panic; `# Safety` as a numbered list on every `pub unsafe fn`.

**Internal comments**:
- Non-obvious low-level concept (riscv encoding, MMU/PTE bits, calling-convention quirk, atomic-ordering rationale, Wasm-spec corner) → comment the *why*. Don't assume the reader has read the privileged spec.
- A `// SAFETY:` saying "preconditions hold" isn't enough — name *which* precondition and *why* this site upholds it; cite the spec section.
- Constants from a spec/manual cite the source (`// Per riscv-privileged §3.1.6.1`).
- Magic numbers → name a `const` with a doc comment.
- `unsafe` blocks manipulating page tables / CSRs / asm registers / trap frames earn 2–3 lines of context.

A non-osdev reader unable to follow the change from comments alone → **Minor** (Major in `sys/kernel`, `lib/riscv`, `lib/trap`).

### Manual book (`manual/src`)

User-visible changes ship a `manual/src/` update in the **same** change: boot args, public syscalls / host functions, public APIs of `sys/loader/api` and consumer crates, build/config knobs, new arches/devices/Wasm proposals. Missing entirely → **Major**; commit message describes it but the book doesn't → **Minor**.

### Change hygiene

- **Description**: subject ≤ 70 chars, prefixed per repo style (`kernel:`, `kasync:`, `loader:`, `build:`, `lib/<crate>:`, `chore:`, `doc:`, `refactor:`, `fix:` — confirm with `git log`). Body explains *why*. Empty/placeholder (`fixes`, `wip`, `(no description set)`) → **Major** — unreviewable.
- **Scope**: one change does one thing. Behavior change hidden in a refactor → **Major**.
- **No accidental files**: `.DS_Store`, debug `dbg!`/`println!`, commented-out code, `TODO: remove`, IDE config, generated artifacts outside `third-party/` → finding. `git diff --name-only main` and scan.
- **Sequential commits** should each be independently buildable / pass `just check`. Tip-only build → **Minor**.

## Output format

```
# Review: <scope>

**Verdict**: Ready / Needs Attention / Needs Work
**Preflight**: passed / failed (<step>) / running

## Findings

### Blocker
- **<file>:<line>** — <rule>. <Trigger.> *Source:* <citation>. *Fix:* <change>.

### Major / Minor / Nit
- ...

## Notes
<Open questions lacking a citation, phrased as questions.>
```

No "Strengths" / "Good points" section.

## Don't

- **Don't fix.** Review surfaces findings; user asks for fixes if they want them.
- **Don't praise** or hedge. Wrong is wrong; say so with a citation.
- **Don't flag rustfmt/clippy** — preflight covers them. Factor preflight failures into the verdict; don't re-report each lint.
- **Don't blanket-flag `unwrap`/`panic!`** — calibrate per the critical-path list above.
- **Don't demand verbose SAFETY comments** — terse is house style. Flag missing/wrong, never short.
- **Don't suggest subjective rewrites** — only substantial simplifications with named wins.
- **Don't speculate without a citation** — no citable basis → **Notes** as a question.
- **Don't review files outside the diff** unless they call a changed unsafe API or share an invariant.
- **Don't summarize the diff.** Lead with the verdict.
