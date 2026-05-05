# `mem-core` behavioral model

A specification of what the `mem-core` crate is *intended* to guarantee, derived
by reading the crate and refined over several rounds of assertion and
architectural review. It is
the reference for new code, for callers in dependent subsystems, and for turning
behavior into executable tests.

This document was last reconciled against the source on 2026-05-22.

## How to read this

- **A** — *Assertion*. A property the crate intends to guarantee. Holds in the
  current code unless a `FIX` says otherwise.
- **INV** — *Invariant*. A cross-cutting property spanning multiple components.
- **FIX** — The current code diverges from the model here. A bug to fix; the
  assertion describes intended behavior, not present behavior.
- **OPEN** — An unresolved design question. The model does not yet commit.
- **D** — *Design direction* (see §12). A committed architectural decision the
  crate is evolving toward but does not yet reflect. Unlike a `FIX`, there is no
  existing code to correct — a `D` item describes a target, not a delta.

`FIX` items are the actionable delta between the code and this model; the §12
`D` items are its intended direction of travel.

---

## §0 Cross-cutting invariants

- **INV-0.1 No implicit coherence.** A page-table mutation becomes observable to
  address translation *only* after an explicit `Flush::flush` / `Arch::fence` /
  `Arch::fence_all`. This holds not just across harts but **on the writing hart
  itself**: until a fence, the CPU may keep using a stale or speculatively
  pre-computed translation indefinitely. Every `map*` / `remap*` /
  `set_attributes` / `unmap` doc-comment states exactly this.
- **INV-0.2 Single-writer mutation.** Every method that mutates a
  `HardwareAddressSpace` takes `&mut self`; structural mutation is statically
  single-threaded. The only intentionally shared, internally-synchronized type
  is `BumpAllocator` (`Mutex<R, _>` inside, `FrameAllocator` methods take
  `&self`).
- **INV-0.3 Page-table memory is reached only through a `PhysMap`.** `Table::get`
  / `Table::set` translate an entry's `PhysicalAddress` through
  `PhysMap::phys_to_virt` before calling `Arch::read` / `Arch::write`. During
  bootstrap the identity case is `PhysMap::ABSENT` (offset 0, `phys == virt`).
- **INV-0.4 `0` is a vacant entry.** `Arch::PageTableEntry` requires the all-zero
  bit pattern to be valid and to mean *vacant*; freshly zeroed frames are
  therefore valid empty page tables.
- **INV-0.5 Mutation is not transactional.** On `Err`, `map` / `map_contiguous`
  may leave the address space partially altered; the documented recovery is for
  the caller to `unmap` the range.

---

## §1 Addresses — `PhysicalAddress`, `VirtualAddress`

Newtype `usize` wrappers (`#[repr(transparent)]`), `Copy`, totally ordered.

- **A-1.1** Arithmetic comes in three explicit flavors with no silent mixing:
  panicking (`add`, `sub`, `offset`, `offset_from_unsigned`), `wrapping_*`, and
  `checked_add` / `saturating_add`.
- **A-1.2** `align_up` / `align_down` / `is_aligned_to` panic unless `align` is a
  power of two. `align_up` rounds up, `align_down` rounds down, both idempotent;
  results are `is_aligned_to(align)`.
- **A-1.3** `offset_from` is the wrapping signed distance;
  `offset_from_unsigned` panics if `self < origin`.
- **A-1.4** `VirtualAddress::is_canonical::<A>()` is true iff every bit at or
  above `A::VIRTUAL_ADDRESS_BITS` equals the sign bit at that position.
  `canonicalize::<A>()` sign-extends from that bit; it is the identity on
  already-canonical addresses and maps the non-canonical hole elsewhere.
- **A-1.5** `from_ptr` / `as_ptr` round-trip exposed provenance; `as_non_null`
  yields `None` exactly for address `0`.

---

## §2 Address ranges — `AddressRangeExt` on `Range<…Address>`

- **A-2.1** `from_start_len(start, len)` produces `start .. start.add(len)`
  (panics on overflow); `len()` of the result equals `len`.
- **A-2.2** `is_empty()` is `start >= end`; `len()` is
  `end.offset_from_unsigned(start)` — i.e. it assumes `start <= end` and will
  panic on an inverted range rather than report 0.
- **A-2.3** `overlaps` is strict (`start < other.end && other.start < self.end`);
  touching ranges do not overlap. `intersect` is `max(start)..min(end)` and may
  yield an empty range.
- **A-2.4** `align_in` shrinks to the aligned sub-range
  (`start.align_up .. end.align_down`); `align_out` grows to the aligned
  super-range. `align_in` of a too-small range yields an empty range.

---

## §3 Memory attributes — `MemoryAttributes`, `WriteOrExecute`

- **A-3.1 W^X is type-enforced.** A region's write/execute permission is a single
  `WriteOrExecute` field (`Neither` / `Write` / `Execute`); writable *and*
  executable is unrepresentable.
- **A-3.2** `allows_read` / `allows_write` / `allows_execution` report the
  `READ` bit and the `WRITE_OR_EXECUTE` variant respectively.
- **A-3.3** `is_read_only()` is true iff the region permits reads **and** permits
  neither writes nor execution.
  - **FIX-1.** `is_read_only` masks only the `READ` bit
    (`self.0 & 1 == 1`); it ignores `WRITE_OR_EXECUTE` and so returns `true` for
    a readable region that is also writable or executable. It should test the
    whole byte (`self.0 == 1`, equivalently
    `allows_read() && !allows_write() && !allows_execution()`).

---

## §4 Architecture abstraction — `Arch`, `PageTableEntry`, `PageTableLevel`

- **A-4.1** `LEVELS` is ordered root→leaf. `GRANULE_SIZE` is the last (smallest)
  level's page size; `VIRTUAL_ADDRESS_BITS` is
  `log2(LEVELS[0].entries) + LEVELS[0].index_shift`. (RISC-V: Sv39→39, Sv48→48,
  Sv57→57.)
- **A-4.2** `PageTableEntry` partitions into exactly three states —
  `is_vacant`, `is_leaf`, `is_table` — mutually exclusive and exhaustive.
  `VACANT` is the all-zero pattern (see INV-0.4).
- **A-4.3** `PageTableLevel::pte_index_of` returns an in-bounds index
  (`< entries`) for any address. `can_map(virt, phys, len)` is true iff `virt`
  and `phys` are aligned to this level's `page_size`, `len >= page_size`, and the
  level `supports_leaf`.
- **A-4.4** `set_active_table` is `unsafe`: after it returns, all non-global,
  non-identity-mapped pointers are dangling. It establishes no ordering and does
  not flush — a caller that changed mappings or reused an ASID must
  `fence_all`.
- **A-4.5** A leaf PTE encodes exactly the supplied `MemoryAttributes`;
  `attributes()` round-trips `new_leaf(addr, attrs)`. `address()` round-trips
  the stored physical address (granule-aligned).
  - **FIX-2.** RISC-V `PageTableEntry::new_leaf` writes `R`, `W`, `X` directly
    from the attributes, so a `WriteOrExecute::Write` region with `READ` unset
    produces `W=1, R=0` — a reserved/illegal RISC-V leaf encoding. `new_leaf`
    should force `R` on when `W` is set (or the attribute model should make
    `Write` imply readable). **D-7 supersedes this:** a writable-without-readable
    region becomes unrepresentable at `MemoryAttributes` construction, so
    `new_leaf` never receives the illegal input.
  - **FIX-3.** Only `Riscv64Sv39` has a public constructor. `Riscv64Sv48` and
    `Riscv64Sv57` carry a private `asid` field with no `new`, so they cannot be
    instantiated outside the module. Add `new(asid: u16)` for both.

---

## §5 Page-table walking — `Table`, `PageTableEntries`

- **A-5.1** `Table` is a `(base, depth)` pair with a `BorrowType` marker
  (`Owned` / `Mut<'_>` / `Immut<'_>`); it owns no memory inline. `level()` is
  `A::LEVELS[depth]`.
- **A-5.2** `Table::get` / `Table::set` access entry `index` at
  `base + index * size_of::<PTE>()` via the physmap (INV-0.3). Both are `unsafe`
  on the in-bounds-`index` precondition; `set` additionally `debug_assert`s it.
- **A-5.3** `visit_mut(range, …, f)` performs a depth-first walk of exactly the
  page-table entries spanning `range`, calls `f` on each, writes the
  (possibly mutated) entry back, and descends into any entry that `f` left as a
  table. It allocates no heap (explicit `ArrayVec` stack, depth ≤ 5).
- **A-5.4** `page_table_entries_for(range, level)` yields
  `(entry_index, sub_range)` pairs covering `range` at `level`, with
  `entry_index` always in bounds and `sub_range` clamped to `range.end`.
- **A-5.5** `Table::is_empty` is true iff *every* entry at this level is vacant.
  - **FIX-4.** `is_empty` initializes `is_empty = true` and folds with `|=`
    (`is_empty |= entry.is_vacant()`), so it returns `true` for *any* table. It
    must fold with `&=`. As written, `unmap` (§6) believes every subtable is
    empty and frees still-in-use page-table frames — memory corruption.
  - **OPEN-2.** `page_table_entries_for` builds `iter` as a
    `RangeInclusive<u16>` of *masked* PTE indices. A `range` that wraps the
    per-level index space (start index > end index after masking) or crosses the
    non-canonical hole yields an empty `RangeInclusive` and the walk silently
    stops early. Is such a `range` a supported input, or a caller precondition
    to document and `debug_assert`? Within `visit_mut`'s per-entry recursion the
    sub-ranges never wrap, so this only bites top-level `unmap` / `lookup` on
    very large ranges.

---

## §6 Address space — `HardwareAddressSpace`

Operations (`map`, `map_contiguous`, `remap`, `remap_contiguous`,
`set_attributes`, `unmap`, `lookup`) are available in both phases; see §11 for
the phase typestate.

- **A-6.1 Map establishes translation.** After `map` / `map_contiguous` returns
  `Ok` *and* the returned-range flush is applied, every access to `virt`
  translates to the corresponding `phys` and obeys `attributes`.
- **A-6.2 Map preconditions (all `unsafe`).** The entire `virt` range must be
  unmapped; `virt` and every `phys` block must be granule-aligned; the `phys`
  blocks must in total be at least as large as `virt`. `map` splits a
  discontiguous `phys` iterator into contiguous `map_contiguous` calls.
  `map_contiguous` `debug_assert`s `len >= GRANULE_SIZE` and the alignments.
- **A-6.3 `map` reuses or creates intermediate tables; leaf slots must be
  vacant.** Where `can_map` holds, a leaf is written; otherwise a zeroed frame
  is allocated for the next level and the walk descends.
  - **FIX-5.** The `map_contiguous` closure `debug_assert!(entry.is_vacant())`
    on *every* visited entry, including intermediate-table entries. Mapping a
    range that shares an already-existing intermediate table debug-panics; in
    release the closure instead overwrites the existing table entry with a
    freshly allocated frame, leaking the old subtable and all mappings beneath
    it. The closure must tolerate an existing *table* entry (descend into it)
    and require vacancy only for *leaf-target* slots.
- **A-6.4 Remap.** `remap` / `remap_contiguous` repoint leaf entries of an
  already-mapped range to new physical memory, preserving each entry's existing
  `attributes`; the old physical memory is not freed. Precondition: the entire
  `virt` range is mapped. `remap_contiguous` is infallible (so `remap` never
  actually returns `Err`, despite its `# Errors` section).
- **A-6.5 Set attributes.** `set_attributes` replaces the `MemoryAttributes` of
  every leaf in an already-mapped range, preserving the physical address.
- **A-6.6 Unmap.** `unmap` clears every leaf in the range and, when a subtable
  becomes empty as a result, frees that subtable's frame via the supplied
  `FrameAllocator` and escalates the flush to `All`. Precondition: the entire
  range is mapped. Requires an allocator that supports `deallocate` — see
  FIX-4 (which currently breaks emptiness detection) and §9 (`BumpAllocator`
  cannot deallocate).
- **A-6.7 Lookup.** `lookup(virt)` walks root→leaf and returns
  `(block_base_phys, attributes, level)` for the first leaf, or `None` at the
  first vacant entry. The physical address is the **block base**; the caller
  must add `virt`'s offset within the `level.page_size()` block.
- **A-6.8** Every mutating operation only *records* the work to flush in the
  caller-supplied `&mut Flush`; nothing is fenced until the caller flushes
  (INV-0.1). The doc-comments' phrase "the returned `[Flush]`" is stale — the
  `Flush` is an in/out parameter, not a return value (doc FIX).

---

## §7 TLB flushing — `Flush`

- **A-7.1** `Flush` accumulates pending invalidations as either a bounded set of
  ranges (`Ranges`) or the whole address space (`All`). `invalidate_all`, and
  any operation that restructures intermediate tables, escalate to `All`;
  escalation is sticky.
- **A-7.2** `flush(arch)` consumes the `Flush` and issues `arch.fence` per range
  (or `arch.fence_all` for `All`), discharging INV-0.1.
- **A-7.3 A `Flush` must be discharged.** Exactly one of `flush` (apply) or
  `ignore` (deliberately skip) should be called on every `Flush`. `ignore` is
  `unsafe` because skipping leaves stale translations.
  - **FIX-6.** `Flush` is not `#[must_use]` and has no `Drop`. Dropping a
    `Flush` silently skips the required fence (INV-0.1 violation with no
    diagnostic). Because there is no `Drop`, `ignore`'s `mem::forget` is a
    no-op of *intent* only. Add `#[must_use]`, and ideally a `Drop` that panics
    (or `debug_assert`s) unless the `Flush` was discharged, making `ignore`
    meaningful.
  - **FIX-7.** `invalidate` calls `ArrayVec::push`, which panics once more than
    `CAP` (default 16) distinct ranges accumulate. Mapping many discontiguous
    blocks can hit this. `invalidate` should degrade to `Flush::All` on
    overflow instead of panicking.
  - **FIX-8.** The const generic `CAP` is effectively dead: `impl Flush` and
    `impl Default for Flush` bind the default `CAP = 16`, so a `Flush<N>` for
    `N != 16` has no constructor or methods. Make the impl blocks generic over
    `CAP`.

---

## §8 Frame allocation — the `FrameAllocator` trait

- **A-8.1** `allocate(layout)` returns an `ExactSizeIterator` of physical blocks
  whose sizes sum to at least `layout.size()`; each block is individually
  aligned to `layout.align()`. Contents are uninitialized. Blocks may be larger
  than requested.
- **A-8.2** `allocate_contiguous(layout)` returns a single block meeting
  `layout`'s size and alignment.
- **A-8.3** `allocate_contiguous_zeroed` allocates then eagerly zeroes the block
  through the physmap; the returned memory is fully zeroed.
- **A-8.4** `allocate_zeroed` allocates then zeroes; the returned memory is
  fully zeroed.
  - **FIX-9.** `allocate_zeroed` zeroes *lazily* via `Iterator::inspect` — a
    block is zeroed only when pulled with `next()`. A caller that drops the
    iterator early receives those blocks as allocated-but-not-zeroed, violating
    the "initialized to zero" contract. See OPEN-1.
- **A-8.5** `deallocate` is `unsafe`: `block` must be currently allocated by
  this allocator and `layout` must fit it.
- **A-8.6** The allocator may be freely copied / cloned / moved (it is meant for
  ZSTs / references / handles); doing so does not invalidate outstanding
  blocks, and `by_ref` yields an equivalent borrowing adapter.
- **OPEN-1.** How should the model enforce that an `allocate_zeroed` /
  `allocate` iterator is fully consumed? Rust cannot forbid an early drop.
  Candidates: zero eagerly before returning; return a drop-guard iterator type
  that zeroes (or releases) unread blocks on `Drop`; or weaken A-8.4 to
  "zeroed only for blocks actually yielded" and document it loudly.

---

## §9 Bump allocator — `BumpAllocator`

- **A-9.1** Constructed from up to `MAX_REGIONS` physical regions; `new` panics
  if any two regions overlap. Each region is shrunk to a granule-aligned
  sub-range (`align_in`); allocation proceeds downward from the top of a region.
- **A-9.2** The largest region is the fast-path "current" arena; the slow path
  scans the remaining arenas (and, for discontiguous `allocate`, splits the
  request across them).
- **A-9.3** Every returned address — and the internal bump pointer — stays
  aligned to at least `GRANULE_SIZE`, and lies within one of the constructed
  regions. Returned blocks never overlap each other.
- **A-9.4** The model constrains only **size, alignment, region-membership, and
  non-overlap** of returned blocks. Exact addresses, which arena a request is
  served from, and split granularity are **unspecified** and must not be relied
  on (in particular they are not deterministic across construction inputs).
- **A-9.5** `BumpAllocator` does not support freeing: `deallocate` is
  `unimplemented!()` and panics. Consequently `unmap` (A-6.6), which frees
  emptied subtables, must not be used with a `BumpAllocator`.
- **A-9.6** `BumpAllocator` is the one shared-mutable component (INV-0.2): all
  `FrameAllocator` methods take `&self` and lock the internal `Mutex`.

---

## §10 Physical memory map — `PhysMap`

- **A-10.1** `PhysMap::new(base, regions)` records a single signed
  `translation_offset = base - min(region starts)`. It panics if `regions` is
  empty, and panics if the offset is zero (an identity-mapped physmap is
  disallowed; that case is `PhysMap::ABSENT`).
- **A-10.2** `phys_to_virt(p) = p + translation_offset` (and `ABSENT` is the
  identity). `phys_to_virt_range` maps both endpoints. In debug builds a stored
  range bounds-checks every translation and panics on an out-of-range physical
  address.
- **A-10.3** `phys_to_virt` is the sole sanctioned way to reach physical memory
  (page-table frames, frames being zeroed) by virtual address — see INV-0.3.

---

## §11 Construction phase — `Bootstrapping` / `Active` typestate

- **A-11.1** `HardwareAddressSpace<A, Phase>` is a typestate: `new` yields
  `Bootstrapping`; `finish_bootstrap_and_activate` (`unsafe`) consumes it and
  yields `Active`.
- **A-11.2** `Bootstrapping`-only operations — `map_identity`,
  `map_physical_memory`, `finish_bootstrap_and_activate` — `debug_assert` that
  the machine has no active page table. During this phase mappings are built
  with no per-step flush (`Flush::ignore`); correctness relies on the single
  `fence_all` inside `finish_bootstrap_and_activate`.
- **A-11.3** `finish_bootstrap_and_activate` calls `set_active_table` then
  unconditionally `fence_all` — load-bearing, especially when an ASID is reused.
- **A-11.4** `Active`-only `from_parts` (`unsafe`) / `into_parts` decompose and
  recompose an address space from `(A, root_table)`.

---

## §12 Architectural direction

Everything above describes `mem-core` as it stands: a RISC-V-only library whose
only consumer is the loader. The decisions below are the **target** the crate is
evolving toward — the single hardware page-table layer for the whole system —
settled over several rounds of architectural review. They are committed
directions, not yet reflected in code.

- **D-1 System-wide bottom layer.** `mem-core` becomes the only hardware
  page-table abstraction in k23. The kernel's hand-rolled `arch/riscv64/mem.rs`,
  its `ArchAddressSpace`, and its `Flush` are retired in favour of `mem-core`'s
  equivalents; the kernel's VMAR/Vmo virtual-memory manager is layered *on top*
  of `mem-core`, not beside it. Consequence: the model is judged against real
  kernel needs — demand paging, MMIO mappings, multiple address spaces, a
  free-capable allocator — not just the loader's one-shot bootstrap.

- **D-2 Cross-architecture `Arch`.** The `Arch` trait must abstract RISC-V
  (Sv39/Sv48/Sv57), AArch64, and x86-64 behind one interface, with no ISA
  specifics leaking to callers. Where the architectures genuinely disagree the
  *backend* absorbs the difference (see D-5, D-7); the generic layer commits to
  the weakest common contract.

- **D-3 Multiple address spaces.** `mem-core` must support more than one live
  `HardwareAddressSpace`. k23 runs a single global address space by default, but
  additional aspaces are created (a) when a root aspace's virtual range is
  exhausted — *aspace paging*: further mappings are placed in a fresh aspace —
  and (b) to hold clusters of provably-disjoint Wasm instances (instances that
  can never import or export to one another). The policy is unfinalized; the
  *mechanism* — N coexisting aspaces, switched between — is required.
  Consequences: the `Bootstrapping`/`Active` typestate (§11) must accommodate
  aspaces created after boot (OPEN-3); ASIDs/PCIDs become load-bearing for
  switch-time TLB economy; and the kernel must stay reachable from whichever
  aspace is active (OPEN-4).

- **D-4 TLB shootdown — CPU-set hint.** `Arch::fence` / `fence_all` stay
  parameterless. The per-aspace arch backend instead carries a mutable *CPU-set
  hint*: the set of CPUs that may hold cached translations for this aspace. It
  defaults to "all CPUs"; a higher layer (Wasm-instance scheduling/affinity) may
  narrow it — ideally updated on every CPU migration. Until narrowed, every
  TLB-invalidating change is conceptually an all-CPU broadcast; broadcast is the
  correct last-resort baseline, not the optimised path. `mem-core` owns the slot
  and the mechanism; the narrowing policy lives above it and is unsettled (it
  interacts with memory shared between Wasm instances — OPEN-5). This hint is
  shared-mutable interior state and becomes a second sanctioned exception to
  INV-0.2, alongside `BumpAllocator`.

- **D-5 Mutation stays overwrite-in-place.** The §6 mutation contract is
  unchanged: a leaf is updated by a single store followed by a flush; the generic
  layer performs no break-before-make. Portability caveat: the AArch64 backend
  must target FEAT_BBM level 2, or itself perform break-before-make for the
  output-address / page-size / memory-type transitions AArch64 forbids on a live
  entry. That risk is carried by the AArch64 backend, not the generic layer.

- **D-6 Memory type — Normal vs Device.** `MemoryAttributes` gains a memory-type
  field with two variants: `Normal` (cacheable) and `Device` (non-cacheable,
  strongly-ordered, for MMIO). Each backend lowers it to its native encoding
  (RISC-V Svpbmt, AArch64 MAIR index, x86 PAT). The split is deliberately minimal
  and may be extended later without reshaping callers.

- **D-7 Fallible leaf encoding.** `PageTableEntry::new_leaf` becomes fallible: a
  backend returns an error for an attribute combination it cannot encode (e.g.
  execute-only on x86-64). Combinations illegal on *every* architecture — notably
  writable-without-readable — are instead made unrepresentable at
  `MemoryAttributes` construction, so they never reach a backend. This supersedes
  FIX-2: RISC-V `new_leaf` no longer needs to repair a reserved `W=1,R=0`
  encoding, because that input can no longer be constructed. Fallibility ripples
  into the error types of `map` / `map_contiguous`.

- **D-8 Accessed/Dirty bits are first-class.** The accessed (A) and dirty (D)
  bits are part of a leaf's modelled lifecycle, not opaque hardware state. A leaf
  has an A/D state; the page-fault handler is part of the mutation story (RISC-V
  Svade, and AArch64 without FEAT_HAFDBS, fault on first access / first write);
  and the Vmo pager reads and clears A/D for working-set and dirty tracking.
  `PageTableEntry` exposes A/D accessors.

- **D-9 Frames are Vmo-owned; the table only borrows.** Physical *data* frames
  are owned by Vmo objects. The page-table layer borrows them: `map` installs a
  borrowed frame and `unmap` clears the PTE — it never deallocates a leaf data
  frame (the Vmo's lifecycle does). `mem-core`'s `FrameAllocator` is therefore
  for page-table *node* frames only: `map` allocates intermediate-table frames
  and `unmap` reclaims them once a subtable is empty — so FIX-4 (`is_empty`) is a
  prerequisite for D-9, and A-9.5 stands (node-frame reclamation needs a
  `deallocate`-capable allocator, which `BumpAllocator` is not). Sharing and
  copy-on-write are expressed entirely in the Vmo layer; `mem-core` has no
  refcounts and no CoW logic — a write-protect fault is resolved by the Vmo layer
  calling back into `mem-core` to remap.

- **OPEN-3.** Runtime aspace construction. A-11.2 `debug_assert`s the machine has
  *no active page table* during `Bootstrapping`. A second aspace created at
  runtime (D-3) is built while another aspace is already active and so cannot use
  that path. What constructor builds and first activates a runtime-created
  aspace?

- **OPEN-4.** Shared kernel mapping. D-3 requires the kernel reachable from every
  aspace. Via shared page-table subtrees (one kernel sub-tree referenced by every
  root), full per-aspace replication, or an arch-global mechanism? This
  constrains `unmap` — it must never reclaim a shared kernel subtree (cf. FIX-4)
  — and interacts with hardware "global" PTE bits.

- **OPEN-5.** CPU-set narrowing policy (D-4): when, and by whom, the per-aspace
  CPU-set hint is updated relative to Wasm-instance migration, and how that stays
  correct for memory shared between instances running on different CPUs.

---

## FIX summary (code ≠ model)

| # | Location | Defect |
|---|----------|--------|
| FIX-1 | `memory_attributes.rs::is_read_only` | Ignores `WRITE_OR_EXECUTE`; true for writable/executable regions. |
| FIX-2 | `arch/riscv64.rs::new_leaf` | Can emit `W=1,R=0`, a reserved RISC-V leaf encoding. Resolution superseded by D-7. |
| FIX-3 | `arch/riscv64.rs` | `Riscv64Sv48` / `Riscv64Sv57` have no public constructor. |
| FIX-4 | `table.rs::is_empty` | `|=` instead of `&=` → always `true` → `unmap` frees in-use frames. |
| FIX-5 | `address_space.rs::map_contiguous` | Closure rejects existing intermediate tables; release build leaks them. |
| FIX-6 | `flush.rs::Flush` | No `#[must_use]` / `Drop`; a dropped `Flush` silently skips the fence. |
| FIX-7 | `flush.rs::invalidate` | Panics on `ArrayVec` overflow instead of degrading to `All`. |
| FIX-8 | `flush.rs` | Const generic `CAP` is dead — impls bound to `CAP = 16`. |
| FIX-9 | `frame_allocator.rs::allocate_zeroed` | Lazy `inspect` zeroing; unconsumed blocks are not zeroed. |

FIX-4 and FIX-5 are soundness bugs (frame corruption / leak). The rest are
correctness or robustness defects.

## Open questions

- **OPEN-1** — Enforcing full consumption of `allocate` / `allocate_zeroed`
  iterators (§8).
- **OPEN-2** — Supported input domain of `page_table_entries_for` for ranges
  that wrap the PTE index space or cross the non-canonical hole (§5).
- **OPEN-3** — How a runtime-created address space is constructed and first
  activated, given the `Bootstrapping` path assumes no active page table (§12).
- **OPEN-4** — How the kernel mapping is shared across every address space (§12).
- **OPEN-5** — When and by whom the per-aspace CPU-set hint is narrowed (§12).

## Notes for turning this into tests

Each `A-*` assertion is an oracle; each `FIX-*` is a currently-failing test to
add. Existing proptests live alongside the code (`address.rs`,
`address_range.rs`, `physmap.rs`, `arch/riscv64.rs`, `frame_allocator/bump.rs`)
and under `proptest-regressions/`; `archtest!` / `for_every_arch!` run a case
across Sv39/Sv48/Sv57.
