# `mem-core` — testing plan

This plan records *how we establish that the crate behaves correctly* — the
test strategy, the tooling it needs, and the families of tests to write, layer
by layer. Working artifact for contributors; revise as the crate and the
tooling evolve.

## 1. Scope

`mem-core` is a virtual-memory subsystem. Its whole value is that the address
translation it builds matches the architecture's translation algorithm and the
documented API contracts. Every component in `src/**` (excluding `test_utils/`)
is in scope. The plan is organised bottom-up along the crate's dependency DAG,
because a layer is only trustworthy once its dependencies are.

## 2. Strategy

### 2.1 Spec vs. implementation — model-based testing

A VM subsystem has a *specification* (the ISA's translation algorithm plus the
contracts of `map`/`unmap`/…) and an *implementation* (page-table edits). The
dominant technique is **model-based / differential testing**: keep an
obviously-correct reference model, apply the same operations to model and
implementation, assert agreement after *every* step.

This mirrors how mature systems are validated:

- **seL4** is verified by *refinement* — abstract spec → executable spec → C,
  each proven to refine the next. We cannot prove, but we borrow the *shape*: a
  layered model the implementation is checked against, with invariants
  re-checked after every operation.
- **RISCOF** certifies a RISC-V core by running identical code on the core and
  on the **Sail golden model**, which implements Sv39/48/57 page-walking from
  the privileged spec. The lesson: a *fully independent* translation oracle
  catches bugs a self-consistent implementation hides.
- **Linux mm** leans on randomized stress and concurrency self-tests — long
  randomized op-sequences plus concurrency model-checking.

### 2.2 The oracle hierarchy

For the address-space layer there are five candidate oracles, in decreasing
independence:

| Oracle | What it is | Independence | Role |
|---|---|---|---|
| Reference model | `BTreeMap<granule-VA, (PA, attrs)>` in test code | total — shares no production code | **primary spec oracle** |
| Independent Sv* spec-walker | test fn walking raw table bytes per the privileged spec | total — shares no production code | validates the table *bytes* are ISA-correct |
| Emulator `Cpu::translate` | the `Machine` TLB walker | partial — `reload_map` reuses production `page_table_entries_for` | realistic execution + TLB semantics |
| `lookup()` | production read path | low — same crate | round-trip target under test |
| Raw PTE inspection | `Table::get` root→leaf, assert bits | n/a | anchors a few golden cases |

Key decision: **the `BTreeMap` model is the primary oracle, never `lookup`.** A
test that checks `map` only against `lookup` passes even if `new_leaf` and
`address()` share a compensating encoding bug. The model and the spec-walker are
what make the differential test real. The emulator is a *secondary* oracle — it
shares `page_table_entries_for`, production code that has itself carried a
serious correctness bug.

### 2.3 Layering

```
L1  address / address_range   pure functions, algebraic laws
L2  memory_attributes         6-state space
L3  arch (PTE, PageTableLevel)encode/decode roundtrips, bit layout
L4  physmap                   translation algebra
L5  frame_allocator / bump    stateful allocator properties
L6  utils (entries_for)       range-tiling properties
L7  table / visit_mut         traversal properties
L8  address_space             model-based core
L9  flush                     accumulator properties
```

### 2.4 Tool selection — Kani / proptest / fuzz / loom / Miri

Each tool is used where it is strongest; overlap is deliberately minimised
("comprehensive but not excessive").

- **Kani proof** — pure, straight-line functions: no unbounded loops, no
  heap-heavy state, input space too large to enumerate. Kani/CBMC reasons over
  *all* bitvector inputs, so a proof holds for every one of 2⁶⁴ values rather
  than a sample. Use for **L1, L1b, the straight-line parts of L3, and L4
  `phys_to_virt`**. These are the layers everything else rests on; proving them
  is both tractable and high-value.
- **proptest** — moderate structured state, bounded loops, heap-backed models,
  where minimal-counterexample shrinking matters and the check must run in the
  per-build `unittests` lane. Use for **L5 single-call properties, L6, L7, L9**,
  and the targeted L8 scenarios.
- **fuzz (libfuzzer + `arbitrary`)** — deep state spaces where coverage-guided
  mutation finds interaction bugs blind sampling misses. The project already
  runs fuzz targets as a **60 s CI gate** per build, and longer on demand, so a
  fuzz target *is* a gate — the deep stateful suites need **no parallel
  bounded-proptest "gate" version**; that duplication is deliberately avoided.
  Use for **L8 op sequences (incl. fault injection) and L5 op sequences**.
- **loom** — concurrency. `adding-tests.md` mandates it for concurrency-touching
  crates. Use for **`BumpAllocator`** (it is `Sync` behind a `Mutex`).
- **Miri** — UB and pointer provenance in `unsafe` code, orthogonal to all of
  the above (Kani proves functional properties; Miri catches UB). Use across
  **L1 pointer methods and the L4/L7 raw-access paths**.

Kani proves the leaves; proptest guards the middle and gates CI; fuzz explores
the deep stateful top; loom does concurrency; Miri does UB.

### 2.5 State / input-space coverage

The VA space is up to 2⁵⁷. **Uniform-random addresses are useless for an
address space** — every operation lands in a disjoint sub-tree, so tables never
interact, coalesce, or get reclaimed. Coverage must be engineered:

- **Dense window** — confine the VAs of one run to a few-GiB window so
  operations overlap, share tables, straddle boundaries, trigger reclamation.
- **Boundary-biased** — explicitly sample addresses adjacent to table
  boundaries at each level and to the canonical hole.
- **Huge-page-aligned** — sample ranges aligned to 2 MiB / 1 GiB so large-leaf
  formation is exercised, not just 4 KiB.
- **Extremes as small separate tests** — top of address space, canonical
  boundary, single-granule ranges.

For the fuzz targets this means a **committed seed corpus** encoding dense,
boundary-adjacent and huge-page-aligned sequences, so the 60 s CI run starts
from good coverage rather than cold.

## 3. Tooling to build

Each item is justified by what it unblocks.

| # | Tool | Why it pulls its weight |
|---|---|---|
| 1 | **Miri-compatible `Memory`** — `cfg(miri)` switches `host_alloc` to the existing `System`-allocator branch instead of `libc::mmap` | Miri has no `mmap`; without this the entire L7/L8 unsafe table-walking code — the riskiest in the crate — is invisible to Miri. ~5 lines. |
| 2 | **Freeing test frame allocator** (`test_utils`) — tracks blocks, supports real `deallocate` | `unmap`, `unmap_inner` and `Table::deallocate` are **100 % untested today** because `BumpAllocator::deallocate` is `unimplemented!()` (correct by design — bump allocators don't free). Nothing can test reclamation without a freeing allocator. Highest-value tool. |
| 3 | **Fault-injecting allocator wrapper** (`test_utils`) — wraps any allocator, returns `AllocError` on a caller-supplied schedule | Partial-failure recovery — cleaning up a `map` that failed partway by `unmap`ping the affected range — is 0 % tested. The only way to drive `map` into its partial-mutation error path deterministically. Consumed by the address-space fuzz target. |
| 4 | **`ModelAddressSpace`** (`test_utils`) — `BTreeMap`-based reference model with the same op set | The independent spec oracle for L8. Shared by the fuzz target. |
| 5 | **Independent Sv* spec-walker** (`test_utils`) — decodes raw table bytes per the privileged spec | The RISCOF-style golden reference. Catches encode/decode bugs that model-vs-`lookup` cannot. Shares zero code with `utils.rs`/`table.rs`. ~40 lines. |
| 6 | **Recording mock `Arch`** (`test_utils`, light) — records `fence`/`fence_all` calls | Lets `flush.rs` tests assert exact fence behaviour without standing up a whole `Machine`. |
| 7 | **Two fuzz targets** — `address_space_ops`, `bump_allocator_ops` — with committed seed corpus + crash artifacts | The deep-coverage CI gate (60 s) and on-demand explorer. Reuse tools 3–5. |
| 8 | **loom scaffold** for `BumpAllocator` | `adding-tests.md` mandates loom for concurrency-touching crates. |
| 9 | **Kani proof harness** — a `rust_kani_proof` BUCK rule (analogous to `rust_loom_test`/`rust_fuzz`), a `just kani` preflight step, and an arch-instantiation macro for monomorphic proofs | Enables L1–L4 *proofs*. Note the integration cost: Kani brings its own toolchain; **validate it builds the crate's nightly-feature set (`step_trait`, `allocator_api`, …) early** before committing. If integration proves intractable, L1/L1b stay as proptests — proof downgrades to sampling, no coverage lost. |

## 4. Test families by layer

Format per family: **[ID] name** — `technique` — reason / behaviour covered.
`for_every_arch!` / `archtest!` is used wherever a property is arch-generic.

### L1 — Address primitives (`address.rs`)

Straight-line arithmetic over one or two `usize` — ideal Kani targets. Run over
both `VirtualAddress` and `PhysicalAddress`.

- **[A1] Overflow regimes** — `Kani` — Proves the three distinct contracts the
  survey calls out: `add`/`sub` wrap-or-panic, `wrapping_*` always wrap,
  `saturating_add` clamps at `MAX`, `checked_add` → `None`, `offset` always
  panics on overflow. Address-space-edge overflow is a classic kernel bug; a
  proof eliminates the whole class.
- **[A2] Arithmetic inverses** — `Kani` — `new`/`get` roundtrip;
  `a.offset(d).offset_from(a) == d`; `wrapping_add`/`wrapping_sub` inverse;
  `From`/`TryFrom` ↔ `get` roundtrips. Covers conversion correctness.
- **[A3] Alignment algebra** — `Kani` — `align_down(x) ≤ x ≤ align_up(x)`; both
  results aligned; idempotence; `is_aligned_to(a) ⇔ align_down(a) == self`.
  Covers the universal granule-alignment quantum.
- **[A4] `align_up` near `MAX`** — `example` + `#[should_panic]` — `align_up`
  uses `wrapping_add` guarded only by a `debug_assert`, so it panics on overflow
  in debug builds and wraps silently in release. Pin both behaviours
  explicitly, because the debug assert masks the release bug from any in-debug
  test (incl. Kani's default config). Justified non-property test: the
  build-mode boundary must be stated.
- **[A5] Non-power-of-two alignment panics** — `#[should_panic]` ×3 —
  `is_aligned_to`/`align_up`/`align_down` panic in every build mode on non-PoT
  align. Covers the input-validation contract.
- **[A6] Canonicalisation** — `Kani`, per-arch — `is_canonical(canonicalize(x))`
  always; `canonicalize` idempotent; `canonicalize(x) == x ⇔ is_canonical(x)`;
  sign bit `VIRTUAL_ADDRESS_BITS-1` correctly replicated. Replaces the three
  existing Sv39-only proptests, generalised over all arches.
- **[A7] `Step` impl** — `Kani` — `forward`/`backward` roundtrip;
  `steps_between` consistent with the `get` difference; `forward_checked` →
  `None` at the boundary. Covers `Range<…Address>` iteration, relied on
  throughout traversal code.

### L1b — Address ranges (`address_range.rs`)

Straight-line over two addresses — Kani targets. Macro over both range types;
include inverted/empty ranges.

- **[R1] `from_start_len` / `len` / `is_empty` consistency** — `Kani` —
  `is_empty ⇔ len()==0 ⇔ start≥end`; `len` is total (never panics on inverted
  ranges — a fixed defect). Subsumes the existing `len`/`len_is_total` proptests.
- **[R2] `contains` half-open semantics** — `Kani` — `contains(start)` ⇔
  non-empty; `!contains(end)`. Covers membership used by `PhysMap` checks and
  the emulator.
- **[R3] `overlaps` / `intersect` duality** — `Kani` — `overlaps` symmetric;
  `overlaps ⇔ !intersect().is_empty()`; `intersect` commutative, idempotent,
  result ⊆ both inputs; disjoint inputs → empty intersection without panic.
  Covers `BumpAllocator::new`'s overlap check and rollback logic.
- **[R4] `align_in` / `align_out` containment** — `Kani` — `align_in ⊆ self ⊆
  align_out`; endpoints aligned; idempotence; already-aligned range is a fixed
  point; `align_in` of a sub-granule range may invert. Covers region alignment.

### L2 — Memory attributes (`memory_attributes.rs`)

- **[M1] Exhaustive predicate table** — `example`, all 6 valid states —
  Enumerating READ∈{0,1} × {Neither,Write,Execute} is *already* a complete
  proof; Kani would add nothing. Asserts every `allows_*` / `is_read_only`
  result and the W^X invariant (`allows_write` and `allows_execution` never both
  true). Subsumes the `is_read_only` regression.

*(The previously proposed test of the `Arbitrary` impl is dropped: it would
test test-only code, and a broken `Arbitrary` impl is self-revealing — every
downstream proptest panics in `get`.)*

### L3 — Arch abstraction (`arch/mod.rs`, `arch/riscv64.rs`)

Bitfield ops over a `usize` — straight-line, Kani-tractable. Per-arch where the
property depends on `LEVELS`.

- **[E1] PTE leaf encode/decode roundtrip** — `Kani` — `new_leaf(addr,attrs)` →
  `address()==addr ∧ attributes()==attrs ∧ is_leaf()`, over the full 56-bit PPN
  range and all 6 attribute states. Proves the leaf-PTE format.
- **[E2] PTE table encode/decode roundtrip** — `Kani` — `new_table(addr)` →
  `address()==addr ∧ is_table()`.
- **[E3] PTE classification is total & exclusive** — `Kani` — over arbitrary
  `usize` patterns, every entry is *exactly one* of vacant/leaf/table; the
  all-zero bit pattern is vacant. Proves the trichotomy every walker
  relies on.
- **[E4] W^X decode panic** — `#[should_panic]` — `attributes()` panics when a
  PTE has both `WRITE` and `EXECUTE`. Covers treating a corrupt PTE as fatal.
- **[E5] PTE bit-layout golden cases** — `example`, ~3 cases cited to the
  RISC-V privileged spec — Hardware-shaped: assert the raw `usize` of a known
  leaf PTE has exactly the spec-mandated bits. Complements the existing
  `const _` static asserts (which check field positions, not end-to-end
  encoding).
- **[E6] `pte_index_of` in-bounds & correct** — `Kani`, per-arch — closes the
  `// TODO: tests` in `mod.rs`. `pte_index_of(a) < entries` and equals
  `(a >> index_shift) & (entries-1)`. An out-of-bounds index here is an OOB
  table access — it breaks the in-bounds-`index` precondition `Table::get` and
  `Table::set` rely on for soundness.
- **[E7] `can_map` decision boundary** — `Kani`, per-arch — `can_map` true ⇔ all
  four conditions hold; each flipped independently. Covers the leaf-vs-descend
  decision in `map_contiguous`.
- **[E8] Const derivations** — `const _` static asserts —
  `VIRTUAL_ADDRESS_BITS` = 39/48/57; `GRANULE_SIZE` = 4 KiB; `GRANULE_LAYOUT`
  well-formed. Compile-time, zero runtime cost.

### L4 — PhysMap (`physmap.rs`)

`phys_to_virt` is straight-line affine arithmetic — Kani-tractable.

- **[P1] `phys_to_virt` is affine, monotone, injective** — `Kani` —
  `phys_to_virt(p) == p + offset`; order-preserving; injective;
  `phys_to_virt_range` preserves length; identity for `IDENTITY`. Subsumes the
  existing `single_region`/`multi_region`/`phys_to_virt` proptests and the two
  half-specific examples.
- **[P2] Construction panics** — `#[should_panic]` ×2 — empty regions; zero
  translation offset ("identity-mapped physmap not allowed").
- **[P3] Out-of-range translation is caught in debug** — `proptest` — physical
  addresses outside the mapped window make `phys_to_virt` panic in debug;
  in-range addresses, including the exclusive range *end*, never panic. Covers
  the physmap's debug-only bounds-check and the deliberately-inclusive upper
  bound.
  (`proptest`, not Kani: depends on the `cfg(debug_assertions)`-only `range`
  field and on panic behaviour.)

### L5 — Frame allocation (`frame_allocator/bump.rs`)

The existing tests do one big allocation each; the gap is **sequences** and
**concurrency**.

- **[F1] Allocation op-sequence** — `fuzz` (`bump_allocator_ops`) — random
  sequence of `allocate`/`allocate_contiguous` with random `Layout`s over a
  random region set. After **every** op assert: (a) returned blocks
  pairwise-disjoint from *all* previously returned blocks — **the critical
  safety property, untested across a sequence today**; (b) every block within
  one configured region; (c) start aligned to `max(layout.align, GRANULE_SIZE)`;
  (d) combined size ≥ `layout.size()` rounded to granule; (e)
  `capacity()+usage()` invariant; (f) the discontiguous `allocate_slow` rollback
  leaves accounting exact on failure. Covers the `FrameAllocator` output
  contract and the disjointness property whose violation hands the same frame
  out twice.
- **[F2] Exhaustion is exact** — `proptest` — allocating exactly total capacity
  succeeds; one more granule → `AllocError`. A focused deterministic check in
  the `unittests` lane.
- **[F3] `new` panics on overlapping regions** — `#[should_panic]` — the
  overlap check has no test today.
- **[F4] Zeroed variants zero** — `proptest` — `allocate_zeroed` /
  `allocate_contiguous_zeroed` blocks read back as zero. (Also checked as an
  `F1` invariant; kept standalone as a fast deterministic check.)
- **[F5] Concurrent allocation disjointness** — `loom` — two threads each
  `allocate`; blocks disjoint, no panic, no deadlock. Covers the `Mutex` +
  `current_arena_hint` races.

*(A `#[should_panic]` test for `deallocate` is dropped: a bump allocator never
freeing is correct by design, not a defect, and pinning an intentional
`unimplemented!()` has no value.)*

### L6 — Range tiling (`utils.rs`)

Coverage is already strong (`entries_tile_the_range_within_aligned_slots`,
`rejects_a_range_crossing_a_table_boundary`). Two additions:

- **[U1] Upper-half & top-of-space ranges** — `proptest`, per-arch — the
  existing tiling test restricts to the canonical lower half so `canonicalize`
  is the identity. Add a sibling exercising upper-half ranges and ranges
  touching `VirtualAddress::MAX`, so the `canonicalize` + `saturating_add` in
  `PageTableEntries::next` is actually tested — the kernel half is where the
  kernel itself lives.
- **[U2] `size_hint` accuracy** — `proptest` — the number of yielded
  `(index, range)` pairs equals the reported `size_hint`. Covers iterator-length
  correctness relied on by callers.

### L7 — Page tables (`table.rs`)

Currently only `is_empty` is tested; `visit_mut` — the traversal workhorse — is
untested directly. All run per-arch on the `Machine` emulator.

- **[T1] `get`/`set` roundtrip** — `proptest` — write an arbitrary PTE at an
  arbitrary in-bounds index, read it back. Covers the physmap-translated raw
  table access — page-table memory is reached only through the physmap; a prime
  Miri target.
- **[T2] `allocate` yields an empty zeroed table** — `proptest` — a freshly
  allocated table is `is_empty` and every entry is the all-zero `VACANT`
  pattern.
- **[T3] `visit_mut` visits in ascending order, tiling exactly** — `proptest` —
  record every `(range, level)` the visitor sees; leaf-level ranges are
  ascending, contiguous, and union to the input. Covers the depth-first
  in-order contract.
- **[T4] `visit_mut` descends into newly-installed tables** — `proptest` — a
  visitor converting vacant→table on first sight must then be re-entered at the
  child level. Covers the "write back, then descend" behaviour `map_contiguous`
  depends on.
- **[T5] `visit_mut` writes back** — `proptest` — mutations to `&mut entry`
  persist (read back via `get`).
- **[T6] `visit_mut` propagates `Err` and stops** — `proptest` — a visitor
  erroring on the Nth call aborts the walk; later entries are not visited.
  Covers the `AllocError` path of `map_contiguous`.
- **[T7] `deallocate` round-trips** — `proptest`, freeing allocator (tool 2) —
  allocate then deallocate a table; the freeing allocator reports the frame
  returned. Covers `Table::deallocate`, unreachable today.

### L8 — Address space (`address_space.rs`) — the core

The model-based heart of the plan. A single fuzz target carries the heavy
invariant battery; a few targeted proptests cover specific scenarios; golden
examples anchor trust.

- **[S1] Address-space op-sequence** — `fuzz` (`address_space_ops`),
  CI-gated 60 s — generate a precondition-respecting sequence of `map` /
  `map_contiguous` / `remap` / `remap_contiguous` / `set_attributes` / `unmap`
  (the executor consults the model (tool 4): `map` only on unmapped ranges, the
  rest only on mapped ranges; shrunk/mutated inputs that violate a precondition
  are skipped). Uses the freeing allocator (tool 2) so `unmap` runs at all.
  After **every** op, the invariant battery:
  - **model agreement** — for every granule in a sampled set, `lookup` phys +
    attrs == model;
  - **spec-walker agreement** — the independent Sv* walker (tool 5), reading
    raw table bytes, agrees with the model — catches encode/decode bugs
    invisible to model-vs-`lookup`;
  - **largest-leaf** — a leaf's level is the largest legal one (`can_map`);
  - **stale-TLB discipline** — before `flush`, an emulator access to a
    newly-mapped page still faults while `lookup` already succeeds; after
    `flush`, both succeed;
  - **permission enforcement** — an emulator write to a read-only page faults;
    W^X holds end-to-end;
  - **data integrity** — a sentinel written via a mapped VA reads back via the
    PA through the physmap;
  - **reclamation** — after `unmap`, emptied intermediate tables are freed and
    allocator `usage()` returns to baseline;
  - **flush-range correctness** — the ranges recorded in `Flush` cover exactly
    the granules whose translation changed, or `All`.
  This single target subsumes map/remap/unmap/set_attributes correctness,
  discontiguous mapping, boundary straddling, the stale-TLB and permission
  integration checks, and reclamation. A committed seed corpus encodes dense,
  boundary-adjacent and huge-page-aligned sequences.
- **[S2] Partial-failure recovery** — folded into `address_space_ops` via the
  fault-injecting allocator (tool 3) — fault-injection bytes in the fuzz input
  drive `map` into `Err(AllocError)` mid-operation; the harness then attempts
  the documented recovery (`unmap` the range) and asserts the space returns to
  fully-unmapped with all frames accounted for. Covers the partial-failure
  recovery contract. **This is expected to surface a finding** — see §8.
- **[S3] Huge-page formation** — `proptest`, per-arch — map a 2 MiB- and a
  1 GiB-aligned contiguous region; `lookup` returns a leaf at the largest legal
  level, not a fan-out of 4 KiB leaves. A fast deterministic check that the
  huge-page case is hit every build, independent of the fuzzer's luck with
  alignment.
- **[S4] Bootstrap lifecycle** — `proptest` + `example` — `new` → root allocated
  & zeroed; `map_identity` → `virt==phys` resolves after activation;
  `map_physical_memory` → every physical region's bytes are reachable through
  the chosen physmap post-activation (this also hardens
  `Machine::bootstrap_address_space`, which every emulator test depends on);
  `from_parts`/`into_parts` roundtrip. Covers the `Bootstrapping → Active`
  typestate path.
- **[S5] Golden raw-table anchor** — `example`, ~2 cases — map one 4 KiB page at
  a known VA; manually walk root→leaf via `Table::get` and assert the PTE bits.
  Independent of both `lookup` and the emulator — anchors the trust tower.

### L9 — TLB flushing (`flush.rs`)

- **[FL1] `invalidate` coarsening** — `proptest` — extend the existing
  regression: with ≤ `CAP` ranges the result is `Ranges` holding exactly them;
  the `(CAP+1)`-th push coarsens to `All`; never panics, never drops a range.
- **[FL2] `flush` dispatch** — `proptest`, recording mock arch (tool 6) —
  `flush` on `Ranges` calls `arch.fence` once per recorded range with exactly
  those ranges; on `All` calls `arch.fence_all` once.
- **[FL3] `invalidate_all` / `ignore`** — `proptest`, mock arch —
  `invalidate_all` → `All` regardless of prior state; `ignore` performs no fence
  calls. Covers the explicit "full flush already coming" escape hatch.

## 5. Cross-cutting coverage

- **Multi-arch** — every arch-generic family runs Sv39/48/57 via
  `for_every_arch!` / `archtest!` (and an arch-instantiation macro for Kani
  proofs). The arches differ only in `LEVELS` depth and `VIRTUAL_ADDRESS_BITS`.
- **Miri** — `just miri` runs the `unittests` lane under Miri. Tool 1 makes the
  `Machine`-backed L7/L8 tests Miri-clean. Miri validates provenance in the
  `address.rs` pointer methods and the raw access in `Memory`/`Table`. Keep
  proptest case counts modest under `cfg(miri)`.
- **Flush discipline, permission enforcement, data integrity** — checked as
  per-step invariants of the `address_space_ops` fuzz target (§4 S1) rather than
  as standalone tests, so they hold across *every* generated state.

## 6. Out of scope (the "not excessive" guard)

- Trivial accessors — `arch()`, `granule_size()`, `address()`, `depth()`,
  `by_ref()`.
- `Display`/`Debug` formatting — cosmetic.
- `Arch::read/write/read_bytes/write_bytes` default impls on the real arch —
  thin `core::ptr` wrappers; exercised transitively through the emulator.
- Non-default `Flush<CAP>` — every `impl` block is bound to the default
  `CAP = 16`, so a custom `CAP` exposes no constructor or methods; untestable by
  construction.
- `Riscv64Sv48`/`Riscv64Sv57` real constructors — none exist (only
  `Riscv64Sv39` has a public `new`); reached through `EmulateArch` +
  `for_every_arch!`.
- The `test_utils` `Arbitrary` impls — test-only; self-revealing if broken.
- Re-deriving arch-generic properties per arch by hand — always via the macros.
- Example tests where a Kani proof or proptest is equivalent.

## 7. Rollout order

1. **Tooling unblockers** — tool 1 (Miri-compat `Memory`), tool 2 (freeing
   allocator), tool 4 (model). Without tool 2, `unmap` cannot be tested at all.
2. **Unblock `unmap`** — T7, and the reclamation invariant of S1 — first-ever
   coverage of table reclamation.
3. **L8 core** — fuzz target `address_space_ops` (S1) with tool 5 (spec-walker);
   then S3, S4, S5.
4. **Allocator** — fuzz target `bump_allocator_ops` (F1); F2, F3, F4; then F5
   (loom).
5. **Unsafe-traversal gap** — T1–T6, U1, U2.
6. **Pure layers, interim** — write A*/R*/E1–E7/P1/P3 as proptests so coverage
   lands immediately; M1, E4, E5, E8, FL* as specified.
7. **Kani harness (tool 9)** — evaluate crate build compatibility early;
   **promote** A*/R*/E1–E3/E6/E7/P1 from proptest to Kani proofs once green,
   deleting the superseded proptests.
8. **Partial-failure** — tool 3 + the S2 fold into the fuzzer; triage the
   expected finding.

## 8. Open findings to confirm

The plan is expected to surface two issues; both are flagged now so they are
not lost in implementation:

- **`unmap` is entirely untested today.** `BumpAllocator::deallocate` is
  `unimplemented!()` (correct for a bump allocator), so any `unmap` that empties
  an intermediate table panics. The three existing `address_space` tests never
  call `unmap`. Tool 2 closes this.
- **The partial-failure recovery contract may be unsound.**
  `unmap_inner` `debug_assert!(!entry.is_vacant())`, but a `map` that fails
  partway leaves vacant leaves inside the range — so the documented recovery
  ("`unmap` the affected range") would trip that assertion. S2 will confirm or
  refute this; if confirmed it is a contract/code defect, not a test to delete.
