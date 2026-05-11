---
name: review
description: Code review tailored to the k23 microkernel. Checks unsafe soundness, panic/alloc discipline on critical paths, RISC-V atomic ordering, async cancellation, MMIO volatility, FFI/ABI, and Wasm sandbox boundaries. Runs `just preflight` in the background and folds the result into the review. Trigger when the user asks to review a change, branch, or diff in this repo.
argument-hint: "[optional: path, crate, or git revision range; defaults to working copy + branch vs main]"
allowed-tools: Read, Grep, Glob, Bash, Agent, WebFetch, WebSearch
---

# Review — k23

Review the current branch (working copy + commits since `main`) for issues that matter in *this* codebase. Be specific, cite sources, and calibrate to k23's actual norms — not textbook ideals.

## Tone

- **Honest and ruthless.** No hedging, no padding, no "looks good overall." If a design choice is wrong, say so and say why — disagreement is the point of review.
- **No praise sections.** Don't highlight "the good parts" or "what works well." The author already knows what they intended; review is about what they missed.
- **Push back on bad ideas.** A junior contributor making a wrong call deserves a clear "this is wrong because…" not a soft "have you considered…". Be respectful but unambiguous.
- **Cite every finding.** Each finding names a source: a spec section (e.g. "RISC-V Privileged §4.2.1"), an RFC, a repo `file:line`, a doc URL, the Rustonomicon section, or the relevant clippy/UCG rule. "Trust me" is not a citation. Findings without a citable basis go in **Notes** as a question, not in **Findings**.

## Scope

- **Default**: `git diff main` (uncommitted + committed-on-branch vs main).
- **With argument**: scope to that path, crate (e.g., `sys/kernel`), or git revision range.
- Use `git diff`, `git log`, `git show <rev>:<path>` to inspect changes. The repo is jj-managed but git-colocated, and review tooling runs in both local and CI contexts — git works in both, jj does not work on CI runners.

## Workflow

1. **Always run commands from inside the nix devshell!**
2. **Capture the diff** — `git diff --stat main` for the file list, then `git diff main` for content. Honor the argument if given.
3. **Kick off preflight in the background early** — `just preflight` (run_in_background=true). It runs clippy + check-fmt + typos + unittests + miri + loom + selftests + buck2-audit + cargo-deny + license-header. Don't block on it.
4. **Read every changed file in full**, not just hunks. Diffs hide invariants. Pull callers via Grep when an unsafe API or signature changes.
5. **Parallel specialist passes** — for non-trivial diffs, fan out subagents (general-purpose, read-only). One per concern. Pick the 2–5 axes the diff actually touches; don't run all of them.
6. **Synthesize** — fold preflight output in once it returns, dedupe, classify severity, emit the report.

## Specialist axes

- **Simplicity & elegance** — substantive structural simplifications only (the maintainer cares about this; treat it as first-class)
- **Unsafe soundness** — every `unsafe { }` and `unsafe fn` in the diff
- **Inline assembly** — `asm!` / `global_asm!` correctness, register/clobber/options, CSR encoding
- **Panic / alloc discipline** — *path-sensitive*, see calibration below
- **Concurrency** — Send/Sync impls, atomic Ordering, lock discipline
- **Async** — cancellation safety, futures across `.await`, drop cleanup
- **Non-local control flow** — trap, Wasm-trap, async-cancel, panic-unwind: edges that bypass normal Rust flow
- **MMIO / volatile** — register access patterns
- **FFI / ABI** — `extern "C"`, `extern "C-unwind"`, `repr(C)`, riscv calling convention
- **Wasm sandbox** — guest memory bounds, host-import trap surface
- **BUCK / build hygiene** — Cargo.toml drift, license headers, reindeer regen
- **Documentation & comments** — public API docs, *and* internal comments explaining osdev/compiler concepts to non-expert contributors
- **Manual book** — user-visible changes ship a `manual/src/` update in the same change
- **Change hygiene** — scope, description, accidental files
- **Tests** — loom for concurrency, miri-clean unsafe, selftests for kernel paths

## Repo-calibrated rules

### Simplicity & elegance — the maintainer cares about this

Treat this as a first-class review concern. Suggest simplifications when there's a **substantial** win — not nits, not subjective rewrites.

A simplification finding is justified when it can:
- **Remove a public type, trait, or generic parameter** that no caller benefits from
- **Collapse two abstractions into one** when they don't carry distinct meaning
- **Eliminate a state machine** that can be expressed as straight-line code
- **Drop a layer of indirection** (`Box`/`Arc`/`Option`/`Result`/`dyn`) when the wrapped form is what callers actually want
- **Replace runtime dispatch with monomorphization** (or vice versa) when it removes branches/types without harming clarity
- **Replace ad-hoc bit manipulation with named consts or `bitflags!`**
- **Reduce parameter count** by extracting a struct or splitting a function with two unrelated jobs
- **Remove dead code** — unused features, fields, branches, error variants
- **Replace a hand-rolled data structure with one already in `lib/`** (`wavltree`, `sharded-slab`, `range-tree`, `arrayvec`, `spin`)

Each simplification finding must include:
- The concrete change (e.g., "merge `Foo` and `Bar`; the only differing field is `flag`, which is always `true` in `Foo`'s callers — verified at `sys/kernel/src/x.rs:42` and `:88`")
- The win in tangible terms (LOC removed, types removed, branches removed, generics removed)
- Any cost (e.g., "loses ability to add a `Foo`-only field later without breaking `Bar` callers")
- Citation: the call sites that justify the simplification

**Do not** suggest:
- Subjective style ("this would read better with a `match`")
- Marginal LOC churn (saving 3 lines by inlining a helper used twice)
- Speculative rewrites ("this could be simpler if you also rewrote X")
- Pattern-matching against another codebase's idioms without a k23-specific reason

For deeper review, consider a parallel pass with the `simplify` skill on the changed files.

Severity: a substantial structural simplification on a hot path or a public API → Major. Local cleanup → Minor.

### Unsafe (high signal — every change deserves attention)

- Every `unsafe { }` block has a `// SAFETY:` comment. **One line is fine** — k23's house style is terse (`// SAFETY: align is checked to be a power of 2`). Missing entirely → Major. Wordy ≠ better.
- Every `unsafe fn` has a `# Safety` doc section listing caller obligations.
- Manual `unsafe impl Send`/`Sync` justifies itself against the type's interior (raw ptrs? `Cell`? non-`Send` fields?).
- For pointer-heavy code, the SAFETY comment names the *specific* UB it rules out (aliasing / alignment / provenance / init / niche). Vague ("seems fine") doesn't count.
- Flag `get_unchecked` / `slice::from_raw_parts` / `Vec::set_len` whose length comes from a generic `size_hint`, `ExactSizeIterator::len`, `Ord`, `Hash`, or `Deref` — safe traits may lie.
- In `unsafe fn` bodies, every unsafe op is in an explicit inner `unsafe { }` (Rust 2024 `unsafe_op_in_unsafe_fn`).

### Inline assembly — highest-risk surface, slow read required

`asm!` / `global_asm!` blocks bypass Rust's safety, the borrow checker, *and* clippy. They warrant a tightly focused inspection independent of the rest of the diff.

Per-block checks:
- **Operand directions** match what the asm actually does. `in` where `inout`/`lateout` is needed is silent UB. Cite the Rust Reference on inline asm operand semantics.
- **Clobber list** is exhaustive. Every register the asm writes (and isn't an output) is in `clobber_abi(...)` or `lateout(reg) _`. Implicit clobbers (e.g., `mstatus` after a CSR write that side-effects through M-mode state) need a comment.
- **`options`**: `pure` only when the asm has no observable side effects beyond outputs; `nomem` only when it doesn't read or write memory; `readonly` only when it reads but doesn't write; `noreturn` only for asm that genuinely doesn't return; `att_syntax`/`raw` set correctly. Default is the safest, but defaults are often wrong for kernel code.
- **CSR access**: verify CSR number/name against the latest RISC-V Privileged Spec — encoding errors are silent. Cite the spec section in the SAFETY comment.
- **Memory ordering**: asm with no `mem` access still doesn't insert fences. If the asm is logically a fence (e.g., `sfence.vma`), the surrounding Rust must not assume reordering protection beyond what the instruction provides. Cite the spec.
- **Trap entry/exit asm** in `lib/trap` and `sys/kernel`: every caller-saved register the trap can clobber is saved before any Rust code runs, in the order the trap frame layout expects. Verify against the `TrapFrame` struct definition — drift between asm and struct is a Blocker.
- **Tail calls from asm into Rust** respect the calling convention: `sp` 16-byte aligned on RISC-V (psABI), `ra` set, `tp` (hart-local) preserved.

**Cross-reference online when needed** — the agent should WebFetch the relevant page if anything in the asm looks unusual. Authoritative sources:
- [RISC-V Privileged Spec](https://riscv.org/specifications/privileged-isa/) — CSRs, traps, fences, SATP, sstatus
- [RISC-V Unprivileged ISA Spec](https://riscv.org/specifications/) — instruction encoding, base ISA, extensions
- [RISC-V psABI](https://github.com/riscv-non-isa/riscv-elf-psabi-doc) — calling convention, register usage
- [Rust Reference: Inline Assembly](https://doc.rust-lang.org/reference/inline-assembly.html) — operand semantics, options, clobbers
- [Rust Unstable Book: `asm`](https://doc.rust-lang.org/unstable-book/library-features/asm.html) — newer features
- The spec evolves; do not assume the asm in the diff matches stale documentation. Verify.

If the agent can't confirm a CSR encoding, an instruction's behavior, or an operand semantic from the spec, that's a finding ("I cannot verify that `csrrw x0, satp, %0` flushes the TLB without an accompanying `sfence.vma` — see RISC-V Privileged §4.2.1 — please confirm or cite the source").

### Panic / alloc — *path-sensitive*

k23 uses `unwrap`/`expect`/`panic!` pragmatically. **Do not blanket-flag them.** Flag only when the call site is reachable from a critical context:

- Trap / exception handlers (`lib/trap`, dispatch in `sys/kernel`)
- Async runtime core (`sys/async`: Park, Notify, Executor, block_on)
- Scheduler core
- Pre-allocator-init paths (early `sys/loader`, kernel init)
- **Page table / virtual memory operations** (`sys/kernel/src/vm`, mapping/unmapping, TLB shootdown, page-fault handling) — a panic here is unrecoverable
- **Kernel integrity / cryptographic verification** — the load-time signature/hash path must not be bypassable via panic-induced fallback
- Hot Wasm guest entry/exit paths

In those contexts, findings include: `unwrap`/`expect` on non-trivial conditions, raw indexing on user-influenced indices, `unreachable!()` on conditions that aren't *structurally* unreachable, `format!`, `Vec::push` without preceding capacity, `Box::new`, `to_vec`, `Arc::new`, collection clone.

Outside those contexts: only flag panics whose trigger condition is plausibly reachable (e.g., `unwrap()` on an `Option` derived from external input). Prefer `debug_assert!` for invariant checks — 254 uses across `sys/` is the established style.

**Direction of travel**: the codebase is pushing toward fewer panics over time. A *new* `unwrap`/`expect`/`panic!` introduced by the diff in `sys/kernel` or `sys/loader`, even off the critical-path list, deserves a Note suggesting a fallible alternative (`?`, `ok_or`, `get`, `checked_*`). Don't escalate to Minor unless the trigger is reachable.

### Concurrency on RISC-V (weakly ordered memory model)

- `Ordering::Relaxed` is only valid for counters with no happens-before. Synchronization requires Acquire/Release or stronger.
- Every `Release` write must have a paired `Acquire` read on the same location, or no synchronization actually happens.
- MMIO config writes followed by a "go" bit need an explicit fence — RISC-V does not order device accesses against normal memory.
- Concurrency-touching code without a corresponding loom test (`just loom`) → Major.

### Async (sys/async / kasync)

- Holding a lock across `.await` → finding (cancellation drops the future; the guarded state is left invalid).
- `select!` arms whose futures hold lossy partial state (half-read buffer, partial transaction) → finding. Hoist long-lived futures outside the loop.
- Drop glue for hardware cleanup must survive cancellation — flag `mem::forget`, `ManuallyDrop`, or early returns that skip it.
- `Park` / `Notify` impls: justify every `unsafe impl Send`/`Sync` against the underlying primitive.

### Non-local control flow — the cross-cutting risk

Three subsystems in k23 escape normal Rust control flow: **CPU exception/trap handling**, **Wasm guest traps**, and **async cancellation/resumption** (plus **panic-induced unwinding** in any code where `panic = "unwind"`). Bugs here are subtle because the compiler can't see the edges, and the Rustonomicon's "safe by construction" arguments break down at these boundaries.

The cross-cutting principle: **assume the next line may never execute.** State that must survive the gap goes through `Drop`, not through code following the call.

- **CPU traps** (lib/trap, sys/kernel trap dispatch): control jumps from arbitrary instruction boundaries into the trap handler with a non-Rust register state. Anything the handler reads from the saved frame must be valid at *every* possible trap point — verify the frame layout matches the asm save sequence (cite the asm file:line and the `TrapFrame` definition). A handler that allocates, locks something held by interrupted code, or panics in unusual states is a Blocker.
- **Wasm traps**: a guest trap unwinds through host code via the runtime's trap mechanism. Host imports must not hold locks, allocations needing explicit free, or any non-`Drop`-cleaned state across calls into JIT-compiled guest code, because a trap may skip the post-call code path entirely. Cite Wasmtime's documented trap semantics for the comparable case.
- **Async cancellation**: dropping a future cancels it at the most recent `.await`. Already covered above; the cross-cutting principle is what to verify.
- **Panic unwinding**: in code that may unwind, a panic between two operations skips the second. Resource handles needing explicit cleanup go in RAII guards. If the crate is `panic = "abort"`, this is moot — verify which panic strategy applies (cite the `Cargo.toml` / BUCK rule).

**Reviewer's checklist** for any function that calls into asm, JIT, `.await`, or a fallible operation:
1. Mentally insert "anything could happen here" between every two statements.
2. For each statement, ask: does the *next* statement's correctness depend on this one having completed? If yes, can the runtime jump past it (trap, unwind, cancel)?
3. If yes, the dependency must be enforced via `Drop`, not source order.

A "acquire — operate — release" pattern where the operate step can trap, panic, await, or be cancelled, and the release is in plain source order rather than `Drop`, is a Blocker if the resource is hardware/lock state, Major if it's memory.

### MMIO / volatile

- All MMIO register access goes through `read_volatile` / `write_volatile`, or a typed wrapper. Plain field access through `&mut` to MMIO is UB.
- New drivers should follow the `lib/uart-16550` pattern.

### FFI / ABI

- `extern "C"` signatures match across the boundary.
- If the foreign side can unwind, use `extern "C-unwind"` — otherwise unwinding is UB.
- `#[repr(C)]` on every type crossing FFI; field order, padding, and enum representation are part of the contract.
- For handcrafted asm in `lib/riscv` and `lib/trap`, verify register save/restore against the riscv calling convention.

### Wasm sandbox (sys/kernel/src/wasm)

- Guest memory accesses re-validate `offset + len ≤ memory.len()` after any potential `memory.grow`.
- Host imports `Result`-return; never panic into the JIT.
- Host and guest pointer provenance stay separate.

### BUCK / build hygiene

- If `third-party/Cargo.toml` changed, `third-party/BUCK` must be regenerated via `just buckify` (reindeer). Cargo-without-BUCK drift → Major.
- New `.rs` files need the 7-line license header from `build/license-header.txt`. Excluded vendored paths: `lib/range-tree`, `lib/sharded-slab`, `lib/wast`.
- Adding/changing a crate's internal deps requires editing the consumer's `BUCK` `deps` list — `just check` catches this.
- For new crates, `manual/src/contributing/adding-a-crate.md` is the authoritative procedure.

### Documentation & comments

k23 attracts contributors who are strong Rust engineers but **not** experts in osdev, riscv, compilers, or low-level systems. Comments are a teaching surface, not just an artifact. Apply both rules:

**Public APIs** (anything `pub` outside a private module):
- Every public item has a doc comment explaining *what it is* and *when to use it*.
- `# Errors` section on every public `fn` returning `Result`, listing what each error variant means.
- `# Panics` section on every public `fn` that can panic (other than `debug_assert!`), naming the precondition.
- `# Safety` section on every `pub unsafe fn`, listing caller obligations as a numbered list.

**Internal comments** (the part that's easy to skip — don't):
- When the code embeds a non-obvious low-level concept — riscv-specific encoding, MMU/PTE bit layout, calling-convention quirk, ABI requirement, atomic-ordering rationale, Wasm-spec corner — add a comment explaining the *why*. Don't assume the reader has read the privileged spec.
- A `// SAFETY:` that says "preconditions hold" is not enough; it should say *which* precondition, *why* this site upholds it, and reference the spec/section if applicable (e.g., "RISC-V Privileged §4.2.1: SATP write requires SFENCE.VMA before next translation").
- New constants pulled from a spec or hardware manual cite the source (`// Per riscv-privileged §3.1.6.1`).
- Magic numbers without a name → rename to a `const` with a doc comment.
- An `unsafe` block manipulating page tables, CSRs, asm registers, or trap frames earns 2-3 lines of context, not one.

If a reviewer with strong Rust skills but no osdev background couldn't follow the change from the comments alone, that's a Minor finding (Major if the change is in `sys/kernel`, `lib/riscv`, or `lib/trap`).

### Manual book (manual/src)

User-visible changes need a `manual/src/` update in the **same** change. "User-visible" includes:
- Boot arguments / kernel command line
- Public syscalls / host functions exposed to Wasm guests
- Public APIs of `sys/loader/api` and other consumer-facing crates
- Build / configuration knobs
- New supported architectures, devices, or Wasm proposals

Missing book updates → Major if the change is documented nowhere; Minor if commit messages describe it but the book doesn't.

### Change hygiene

A reviewable change is well-scoped *and* well-described. Check the change as a unit, not just the code in it.

- **Description**: `git log -1 --format=%B` for the tip commit; `git log main..HEAD --format=%B` for the series. Each message should let a reviewer read it in 30 seconds and know what to expect.
  - First line is a concise subject (≤ 70 chars), prefixed with the affected component per existing repo style: `kernel:`, `kasync:`, `loader:`, `build:`, `lib/<crate>:`, `chore:`, `doc:`, `refactor:`, `fix:`. (Cite recent commits via `git log` to confirm the convention.)
  - Body, when present, explains *why*, not *what* — the diff already shows what.
  - Empty or placeholder descriptions (`fixes`, `wip`, `(no description set)`) → Major. The change is unreviewable.
- **Scope**: one change does one thing. Refactor + behavior change + dep bump in one commit → split. Mixed-purpose changes are Minor; Major if a behavior change is hidden inside what looks like a refactor.
- **No accidental files**: stray `.DS_Store`, debug `dbg!`/`println!` calls, commented-out code, `TODO: remove before merge`, generated artifacts not under `third-party/`, IDE config files → finding. Run `git diff --name-only main` and scan.
- **Test coverage moves with the change**: a behavior change without a test (selftest, unit, loom, miri) is Minor; concurrency change without loom is Major (already covered).
- **Sequential commits** in a series should each be independently buildable and pass `just check`. If only the tip builds, the series isn't reviewable per commit — Minor finding.

### Style — mostly hands-off

- rustfmt is enforced by preflight (`group_imports = "StdExternalCrate"`, `imports_granularity = "Module"`).
- clippy is enforced by preflight.
- Don't restate what those tools say. Only flag style if it substantively obscures intent.

## Output format

```
# Review: <scope>

**Verdict**: Ready / Needs Attention / Needs Work
**Preflight**: passed / failed (<which step>) / running

## Findings

### Blocker
- **<file>:<line>** — <rule>. <Concrete trigger or scenario.> *Source:* <spec §, RFC, repo file:line, or doc URL>. *Fix:* <suggested change>.

### Major
- ...

### Minor
- ...

### Nit
- ...

## Notes
<Open questions where I lack the citation to make it a Finding. Phrase as a question, not a verdict — e.g., "Is the new `Notify::wait` covered by a loom test? I don't see one in `sys/async/tests/loom.rs`.">
```

**No "Strengths" / "Good points" / "What works well" section.** The author wrote the diff; they don't need to be told what they got right.

**Severity**:
- **Blocker** — UB, sandbox escape, soundness hole, missing/wrong SAFETY on pointer-heavy unsafe, asm/`TrapFrame` drift, deadlock, lost wakeup, license header missing on a non-vendored file, trap/cancel/unwind path that leaks resource state.
- **Major** — panic/alloc reachable on a critical path, missing volatile on MMIO, unjustified `unsafe impl Send`/`Sync`, async cancellation hazard, BUCK ↔ Cargo drift, concurrency change without loom coverage, substantial structural simplification missed on a hot path or public API, empty/placeholder change description, hidden behavior change in a refactor.
- **Minor** — docs gap on public unsafe API, missing `# Safety` section, missing test for a new branch, local simplification missed, mixed-purpose change, missing manual book update where commit messages describe the user-visible part, non-osdev-friendly internal comments in `sys/kernel`/`lib/riscv`/`lib/trap`.
- **Nit** — naming, doc polish, redundancy.

Every finding **cites a source**: `file:line`, spec section, RFC, doc URL, Rustonomicon section, UCG link, or commit hash. Findings without a citable basis go in **Notes** as questions. "Looks fishy" without a citation is not a finding.

## Anti-patterns — do NOT

- **Don't praise.** No "good use of X here", "nice refactor", "this is well-structured". Skip the strengths section entirely.
- **Don't hedge.** "Maybe consider possibly looking into…" → "this is wrong because <reason>; fix: <X>".
- **Don't flag formatting or clippy lints** — preflight covers them.
- **Don't blanket-flag `unwrap`/`panic!`** — calibrate to the critical-path list above.
- **Don't demand verbose SAFETY comments** — terse one-liners are house style. Presence and accuracy, not length.
- **Don't suggest subjective rewrites** — only substantial simplifications with named wins. No "this would read better as…" without a concrete LOC/type/branch reduction.
- **Don't speculate without a citation** — if you can't point to a spec, RFC, repo file:line, doc URL, or established rule, the observation is a question for **Notes**, not a Finding.
- **Don't review files outside the diff** unless they call into changed unsafe APIs or have a direct invariant link.
- **Don't summarize what the diff does** — the author wrote it. Lead with the verdict.
- **Don't soften disagreement.** If the design is wrong, say so directly with the citation. Junior contributors deserve a clear "this is wrong" more than they deserve a comfortable "have you considered".
