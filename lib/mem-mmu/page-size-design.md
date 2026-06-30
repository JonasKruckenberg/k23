# Explicit page-size control for `HardwareAddressSpace`

Investigation note. Draft — relocate into `manual/` when settled.

## 1. The problem with "automagic" largest-page mapping

Today `HardwareAddressSpace::map_contiguous` never takes a page size. It walks the
table top-down (`Table::visit_mut`) and, at every entry, the closure asks
`PageTableLevel::can_map(virt, phys, len)`:

```rust
// lib/mem-mmu/src/address_space.rs  (map_contiguous closure)
if level.can_map(range.start, phys, range.len()) {
    *entry = A::PageTableEntry::new_leaf(phys, attributes);   // place leaf HERE
    ...
} else {
    // allocate an intermediate table and descend one more level
}
```

```rust
// lib/mem-core/src/arch/mod.rs
pub fn can_map(&self, virt, phys, len) -> bool {
    let page_size = self.page_size();
    virt.is_aligned_to(page_size) && phys.is_aligned_to(page_size)
        && len >= page_size && self.supports_leaf
}
```

Because `visit_mut` visits levels largest-first, the **first** level whose page
size both addresses happen to be aligned to wins. The granularity is therefore an
emergent property of address alignment, not a decision the caller can state.

### What that forces on callers (point 1, verified)

* **WASM linear memory** (`sys/kernel/src/wasm/vm/instance_alloc.rs`) wants 2 MiB
  pages, so it aligns the *allocation* to coax them out:
  ```rust
  let align = cmp::min(2 * 1048576, aspace.lock().frame_alloc.max_alignment());
  Mmap::new_zeroed(aspace.clone(), request_bytes, align, None)
  ```
  This is the canonical "align to control granularity" hack — and it's only
  best-effort: if the allocator can't hand back a 2 MiB-aligned frame it silently
  falls back to 4 KiB and the caller never knows what it got.
* **Physmap bounds** (`sys/loader-common/src/lib.rs:214`) are widened to 2 MiB —
  ```rust
  range_phys.align_out(2097152) // TODO remove
  ```
  i.e. the *extent* the physmap covers is rounded out to 2 MiB purely so the
  automagic promotes the physmap mapping to 2 MiB leaves. It's already tagged
  `// TODO remove`: the canonical example of geometry being smuggled in through
  alignment, in a function whose job is supposed to be "compute the phys range",
  not "pick a page size".
* **Loader** (`sys/loader-common/src/mapping.rs`) aligns everything it maps to
  `aspace.granule_size()`. Note `granule_size() == A::GRANULE_SIZE ==
  LEVELS.last().page_size()` — the **smallest** page (4 KiB), the documented
  "translation granule". So the loader is only satisfying `map_contiguous`'s
  *minimum*-alignment precondition; it then *hopes* the automagic promotes the
  kernel image / physmap to 2 MiB/1 GiB leaves. It cannot guarantee a level.
* **Kernel runtime** (`mem/address_space.rs` `Batch`, `AddressSpaceRegion::commit`)
  commits on-demand strictly one `arch::PAGE_SIZE` (4 KiB) page at a time. Large
  pages are simply **unreachable** from the runtime API — the automagic gives it
  nothing.

So the automagic gives the one caller that wants large pages no guarantee, and the
one path that runs constantly no access at all. The abstraction hides the decision
at exactly the points where we want to make it.

### Conclusion for point 1

Keep the largest-fit walk, but demote it to one *explicit, opt-in* mode
(`*_auto`) and add a path where the caller **names** the leaf size and the type
system proves the arch supports it. The hacks (2 MiB alignment, granule rounding)
become `map::<Size2MiB>` / `map::<Size4KiB>` calls.

## 2. What Theseus actually does (points 2 & 3)

Theseus's `memory_structs` crate:

```rust
pub trait PageSize: Ord + Copy + private::Sealed + 'static {
    const SIZE: MemChunkSize;     // enum {Normal4K, Huge2M, Huge1G}
    const NUM_4K_PAGES: usize;
    const SIZE_IN_BYTES: usize;
}
pub struct Page4K; pub struct Page2M; pub struct Page1G;
pub struct Page<P: PageSize = Page4K>   { number: usize, size: PhantomData<P> }
pub struct Frame<P: PageSize = Page4K>  { number: usize, size: PhantomData<P> }
pub struct PageRange<P: PageSize = Page4K>(RangeInclusive<Page<P>>);
```

with `TryFrom<Page<Page4K>> for Page<Page2M>` (checks alignment) and the
infallible `From<Page<Page2M>> for Page<Page4K>` widening→narrowing direction.
The page-table indices are fixed-shift methods on the page number:
`p4_index = (n >> 27) & 0x1FF`, … `p1_index = n & 0x1FF`.

**Two things matter for us:**

1. Those `pX_index` methods bake in a **4-level, 9-bit-per-level, x86_64-only**
   hierarchy. k23 supports Sv39/Sv48/Sv57 (3/4/5 levels) per arch *type*, so a
   given size sits at a **different depth in each mode** (2 MiB is depth 1 under
   Sv39, depth 2 under Sv48, depth 3 under Sv57). We cannot attach
   `pX_index` to the address; the per-level `index_shift` already in
   `PageTableLevel` is the right per-(arch,level) datum.
2. **Theseus does not actually specialize the walk by `P` at compile time.** The
   mapper's huge-page arm is unimplemented (`// TODO FIXME: implement huge pages
   here`); the live path always descends P4→P3→P2→P1 and writes the leaf at P1.
   `P` is **type-state** threaded through the allocator (`AllocatedFrames<P>`,
   `MappedPages`) to track sizes and enforce alignment at conversion boundaries —
   it is *not* a walk optimization.

   Also note Theseus's `NUM_4K_PAGES = … * ENTRIES_PER_PAGE_TABLE` assumes a
   uniform 512-entry radix. That's false on AArch64 16 KiB/64 KiB granules
   (entries-per-table vary by granule and level — see the `PageTableLevel::entries`
   doc-comment). So size→bytes ratios must **not** live in the size marker; they
   must come from the per-arch level list.

### Answer to point 3 (compile-time level optimization)

Yes, and it's k23-original — Theseus gives us the *type-state* idea, not the
*walk-specialization*. The lever is: if **both** `A: Arch` and the leaf size are
generic params, then the leaf **depth is a compile-time constant** (a const lookup
into `A::LEVELS`). With the depth const-known we can:

* drop `can_map` and the leaf-vs-table branch from the hot path — the leaf level is
  predetermined;
* turn the `ArrayVec<_, 5>` DFS stack + iterator bookkeeping into a fixed
  `0..DEPTH` descent the compiler fully unrolls;
* const-fold every `index_shift`/mask (`pte_index_of` becomes `(addr >> CONST) &
  CONST`) and the range step (`S::BYTES`).

Honest bound on the win: the `DEPTH` dependent PTE reads to walk the tables are
runtime data and remain. We remove *bookkeeping, branching and non-const
arithmetic*, and unlock unrolling — largest for shallow/huge-page depths (a 1 GiB
leaf is depth 0 under Sv39: index the root table, write one entry, done), marginal
for 4 KiB. The bigger payoff is correctness/clarity: the type guarantees the level,
killing the alignment-hack dance. Cost: one monomorphized walker per `(A, S)` —
cheap. The compile-time depth is only available because k23 keeps `A` static
(`A: Arch`, never `dyn`); that already holds everywhere.

## 3. Unifying `LEVELS` with a typed `PageSize` (point 4)

The tension: `LEVELS` is **runtime, arch-specific geometry** (needed by `lookup`,
which must walk all levels because the found level is data-dependent, and by the
auto-fit path). A typed `PageSize` is a **compile-time selector**. Don't duplicate
geometry into the trait, and don't delete the array — instead make the type a
typed index into the array, and **generate both from one source so they can't
drift.**

The insight: *a leaf-capable level **is** a supported page size.* They're the same
fact seen from two angles. So:

### a. Size markers (arch-independent, carry only the size)

```rust
// lib/mem-core: closed set, named by SIZE (mode-independent), geometry-free
pub trait PageSize: Copy + Ord + 'static + sealed::Sealed {
    const SHIFT: u8;                       // log2(bytes)
    const BYTES: usize = 1 << Self::SHIFT; // 4KiB..256TiB
}
// Size4KiB(12) Size2MiB(21) Size1GiB(30) Size512GiB(39) Size256TiB(48)
```

Named `Size2MiB`, not `Level1`, precisely because the *depth* is per-mode. The
marker says "which size"; it deliberately knows nothing about entries/depth.

**Naming is arch-independent — no cfg at any callsite, no per-arch re-export.**
The markers are defined once in `mem-core` and re-exported from `mem-mmu`; callers
write `use mem_mmu::Size2MiB;` on every target. They must *not* be arch-tagged: a
byte size is a byte size on every arch, and the arch is already carried by the
address space, not the size. `map_contiguous::<Size2MiB>` is a method on
`HardwareAddressSpace<A>` gated by `where A: MapsAt<Size2MiB>`, so a riscv size
marker cannot leak into an aarch64 space — `A` prevents it. Arch-tagging the marker
(or re-exporting a same-named type from each `arch/*.rs`) would be redundant *and*
would drag cfg back into calling code; don't. The only arch-specific things are the
`define_arch_levels!` invocation and the `MapsAt<S>` impls it generates.

The *set* of sizes is arch-dependent in full generality (AArch64 16 KiB/64 KiB
granules have 32 MiB / 512 MiB blocks, not 2 MiB / 1 GiB) — but that's just *more
global names*, not cfg. Every current target (riscv64 Sv39/48/57, x86_64,
aarch64-4K) shares `{Size4KiB, Size2MiB, Size1GiB, Size512GiB, Size256TiB}`, so
they share names with zero cfg. A future aarch64-16K would add `Size32MiB` &c. as
further global markers, gated into validity by `MapsAt`. (Same as Theseus defining
`Page4K/2M/1G` globally regardless of which are x86-only.)

### b. The bridge trait (one fact: "arch A has a LEAF level for size S, at DEPTH")

```rust
pub trait MapsAt<S: PageSize>: Arch {
    const DEPTH: u8;   // depth (root = 0) of the S-sized leaf level
}
```

* **Validity is a trait bound.** `map::<Size512GiB>` on `Riscv64Sv39` fails to
  compile because `Riscv64Sv39: MapsAt<Size512GiB>` doesn't exist — a clean
  unsatisfied-bound error, far better than a post-monomorphization const-panic.
* `DEPTH` is the const that feeds the §2.3 specialized walk.

### c. Keep the readable `LEVELS` array; derive `DEPTH` from it — one source, no macro magic

`LEVELS` stays hand-written — it is the readable source of truth that answers "how
do we know the specs", and `lookup`/auto-fit walk it at runtime. Two things make it
also feed the typed layer without drift:

1. `PageTableLevel::new` takes the size as a **`PageSize` type parameter**, so a
   level's geometry is tied to its named size (its `index_shift` *is* `P::SHIFT`):

   ```rust
   const LEVELS: &'static [PageTableLevel] = &[
       PageTableLevel::new::<Size1GiB>(512, true),
       PageTableLevel::new::<Size2MiB>(512, true),
       PageTableLevel::new::<Size4KiB>(512, true),
   ];
   ```

2. A `const fn` derives each `MapsAt::DEPTH` *from* `LEVELS`, so depth is never
   hand-typed and cannot disagree with the array:

   ```rust
   pub const fn leaf_depth_of<A: Arch, S: PageSize>() -> u8 {
       let mut d = 0;
       while d < A::LEVELS.len() {
           let lvl = &A::LEVELS[d];
           if lvl.page_size() == S::BYTES && lvl.supports_leaf() { return d as u8; }
           d += 1;
       }
       panic!("architecture has no leaf page-table level for this page size")
   }

   // one line per arch, listing the leaf sizes it supports:
   impl_maps_at!(Riscv64Sv39: Size1GiB, Size2MiB, Size4KiB);
   //  expands to `impl MapsAt<Size1GiB> for Riscv64Sv39 { const DEPTH = leaf_depth_of::<…>(); }` …
   ```

`impl_maps_at!` is a trivial non-recursive macro (no depth counting). Listing a size
that isn't a leaf level in `LEVELS` makes `leaf_depth_of`'s `panic!` fire at
**compile time** (it initialises a `const`). Intermediate levels that can't hold a
leaf (some AArch64 configs) are just constructed with `supports_leaf = false` and
left out of `impl_maps_at!`. This is the unification: **`LEVELS` is the runtime
engine and the one place geometry is written; the markers are its compile-time
façade; `new::<P>` ties each level to its size and `leaf_depth_of` projects the
array into the typed bridge.** Nothing is removed.

## 4. Proposed API shape

```rust
impl<A: Arch> HardwareAddressSpace<A> {
    /// Force a leaf size; arch must support it (compile-time checked).
    pub unsafe fn map_contiguous<S: PageSize>(
        &mut self, virt: Range<VirtualAddress>, phys: PhysicalAddress, ...,
    ) -> Result<(), AllocError>
    where A: MapsAt<S>;            // unsupported (arch,size) ⇒ compile error

    /// Today's largest-fit behaviour, now explicitly opt-in.
    pub unsafe fn map_contiguous_auto(&mut self, ...) -> Result<(), AllocError>;
}
```

Callers stop aligning-to-control: WASM → `map_contiguous::<Size2MiB>` (and gets a
hard error, not a silent 4 KiB fallback, if the frame isn't 2 MiB-aligned); the
runtime committer → `map_contiguous::<Size4KiB>`; the loader keeps `_auto` for
physmap where best-effort really is the intent, and forces sizes where it cares.

Optional layer 2 (later, bigger refactor): a size-typed `Page<S>`/`Frame<S>` whose
construction proves `S`-alignment (à la Theseus), so the size flows in the type
through the allocator instead of being a turbofish at the `map` call. Defer — the
method-generic form above already removes the hacks and unlocks the §2.3
optimization; typed addresses are additive on top.

## 5. Open questions

* Mixed-size ranges: keep them as a loop of per-size `map_contiguous::<S>` calls in
  a higher layer, or add a `map_best_fit` that emits a *descending* sequence of
  forced calls? Probably the former — keeps the engine simple.
* Should `_auto` be retained at all, or replaced by an explicit size-list the
  caller passes? `_auto` is genuinely useful for "map this physmap however is
  cheapest"; keep it, but make every *other* caller name a size.
* `unmap`/`lookup` stay level-generic (runtime walk) — they must handle whatever
  sizes are actually present, so they keep using `LEVELS` directly.
