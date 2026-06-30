// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![expect(clippy::undocumented_unsafe_blocks, reason = "its fine for benchmarks")]

use std::alloc::Layout;
use std::cell::Cell;
use std::hint::black_box;
use std::iter;
use std::marker::PhantomData;
use std::range::Range;

use criterion::measurement::WallTime;
use criterion::{
    BatchSize, BenchmarkGroup, Criterion, Throughput, criterion_group, criterion_main,
};
use mem_core::arch::riscv64::{Riscv64Sv39, Riscv64Sv48, Riscv64Sv57};
use mem_core::arch::{Arch, MapsAt, PageTableLevel};
use mem_core::{
    AddressRangeExt, AllocError, FrameAllocator, MemoryAttributes, PageSize, PhysMap,
    PhysicalAddress, Size1GiB, Size2MiB, Size4KiB, VirtualAddress,
};
use mem_mmu::{Flush, HardwareAddressSpace};
use mem_testkit::Memory;

const GRANULE: usize = 4096;
const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;
const GIB: usize = 1024 * MIB;

/// Base of the mapped *virtual* range.
const VBASE: usize = GIB;
/// Base of the mapped *physical* range.
const PBASE: usize = GIB;

/// Warm single-page commits timed per iteration of the `commit` workload. 511 fills the
/// rest of the one 4 KiB-level table the pre-mapped page 0 created.
const COMMIT_PAGES: usize = 511;

// ---------------------------------------------------------------------------
// Flat, host-safe `Arch`
// ---------------------------------------------------------------------------

/// Wraps a real RISC-V `Arch` `A` for its geometry (`LEVELS`, PTE type, derived
/// consts) but overrides only the handful of methods that would otherwise touch real
/// hardware, so the walk runs on the host with raw-pointer PTE accesses.
struct FlatArch<A>(PhantomData<A>);

impl<A> FlatArch<A> {
    fn new() -> Self {
        Self(PhantomData)
    }
}

impl<A: Arch> Arch for FlatArch<A> {
    type PageTableEntry = A::PageTableEntry;

    const LEVELS: &'static [PageTableLevel] = A::LEVELS;
    const DEFAULT_PHYSMAP_BASE: VirtualAddress = A::DEFAULT_PHYSMAP_BASE;

    fn active_table(&self) -> Option<PhysicalAddress> {
        None
    }

    unsafe fn set_active_table(&self, _address: PhysicalAddress) {}

    fn fence(&self, _range: Range<VirtualAddress>) {}

    fn fence_all(&self) {}
}

// Delegating bridge: `FlatArch<A>` maps a leaf size at exactly the depth `A` does, so
// `map_contiguous::<S>` is available for every `(mode, size)` the wrapped arch supports.
impl<A, S> MapsAt<S> for FlatArch<A>
where
    A: MapsAt<S>,
    S: PageSize,
{
    const DEPTH: u8 = <A as MapsAt<S>>::DEPTH;
}

// ---------------------------------------------------------------------------
// Bump frame allocator
// ---------------------------------------------------------------------------

struct BumpAlloc {
    region: Range<PhysicalAddress>,
    next: Cell<PhysicalAddress>,
}

impl BumpAlloc {
    fn new(region: Range<PhysicalAddress>) -> Self {
        Self {
            next: Cell::new(region.start),
            region,
        }
    }

    fn bump(&self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        let start = self.next.get().align_up(layout.align());
        let end = start.add(layout.size());
        if end > self.region.end {
            return Err(AllocError);
        }
        self.next.set(end);
        Ok(start)
    }
}

// Safety: every frame is carved from the single live `region` (backed by a `Memory`
// the caller keeps alive); the cursor advances monotonically so no frame is handed out
// twice; `deallocate` is a no-op, which is sound because the benches never free.
unsafe impl FrameAllocator for BumpAlloc {
    fn allocate(
        &self,
        layout: Layout,
    ) -> Result<impl ExactSizeIterator<Item = Range<PhysicalAddress>>, AllocError> {
        let base = self.bump(layout)?;
        Ok(iter::once(Range::from(base..base.add(layout.size()))))
    }

    fn allocate_contiguous(&self, layout: Layout) -> Result<PhysicalAddress, AllocError> {
        self.bump(layout)
    }

    unsafe fn deallocate(&self, _block: PhysicalAddress, _layout: Layout) {}
}

// ---------------------------------------------------------------------------
// Per-iteration world
// ---------------------------------------------------------------------------

/// One fresh, empty address space plus the allocator and physmap that drive it.
struct World<A: Arch> {
    bump: BumpAlloc,
    physmap: PhysMap,
    aspace: HardwareAddressSpace<FlatArch<A>>,
}

/// Builds a fresh address space over `region`.
fn fresh_world<A: Arch>(region: Range<PhysicalAddress>) -> World<A> {
    let bump = BumpAlloc::new(region);
    let physmap = PhysMap::new_identity::<Size4KiB>(iter::once(region));
    let aspace = HardwareAddressSpace::new(FlatArch::<A>::new(), &physmap, &bump)
        .expect("root page-table allocation");
    World {
        bump,
        physmap,
        aspace,
    }
}

fn attrs() -> MemoryAttributes {
    // Attribute bits do not change the walk shape; read-only keeps it simple.
    MemoryAttributes::new().with(MemoryAttributes::READ, true)
}

/// Maps the single `S`-sized page at slot `index` (one full root-to-leaf walk).
fn map_one<A, S>(w: &mut World<A>, index: usize)
where
    A: Arch + MapsAt<S>,
    S: PageSize,
{
    let off = index * S::BYTES;
    let virt = <Range<VirtualAddress>>::from_start_len(VirtualAddress::new(VBASE + off), S::BYTES);
    let phys = PhysicalAddress::new(PBASE + off);
    let mut flush = Flush::new();
    unsafe {
        w.aspace
            .map_contiguous::<S>(
                black_box(virt),
                black_box(phys),
                attrs(),
                w.bump.by_ref(),
                &w.physmap,
                &mut flush,
            )
            .unwrap();
    }
}

/// Maps a contiguous run of `n_leaves` `S`-sized pages in a single call.
fn map_bulk<A, S>(w: &mut World<A>, n_leaves: usize)
where
    A: Arch + MapsAt<S>,
    S: PageSize,
{
    let virt =
        <Range<VirtualAddress>>::from_start_len(VirtualAddress::new(VBASE), n_leaves * S::BYTES);
    let phys = PhysicalAddress::new(PBASE);
    let mut flush = Flush::new();
    unsafe {
        w.aspace
            .map_contiguous::<S>(
                black_box(virt),
                black_box(phys),
                attrs(),
                w.bump.by_ref(),
                &w.physmap,
                &mut flush,
            )
            .unwrap();
    }
}

// ---------------------------------------------------------------------------
// Workload runners (generic over the arch mode)
// ---------------------------------------------------------------------------

fn run_commit<A>(g: &mut BenchmarkGroup<'_, WallTime>, mode: &str, mem_bytes: usize)
where
    A: Arch + MapsAt<Size4KiB>,
{
    let memory = Memory::new::<A>([Layout::from_size_align(mem_bytes, GRANULE).unwrap()]);
    let region = memory.regions().next().expect("one region");

    g.throughput(Throughput::Elements(u64::try_from(COMMIT_PAGES).unwrap()));
    g.bench_function(mode, |b| {
        b.iter_batched_ref(
            || {
                let mut w = fresh_world::<A>(region);
                map_one::<A, Size4KiB>(&mut w, 0);
                w
            },
            |w| {
                for i in 1..=COMMIT_PAGES {
                    map_one::<A, Size4KiB>(w, i);
                }
            },
            BatchSize::PerIteration,
        );
    });
}

fn run_bulk<A, S>(
    g: &mut BenchmarkGroup<'_, WallTime>,
    mode: &str,
    n_leaves: usize,
    mem_bytes: usize,
) where
    A: Arch + MapsAt<S>,
    S: PageSize,
{
    let memory = Memory::new::<A>([Layout::from_size_align(mem_bytes, GRANULE).unwrap()]);
    let region = memory.regions().next().expect("one region");

    g.throughput(Throughput::Elements(u64::try_from(n_leaves).unwrap()));
    g.bench_function(mode, |b| {
        b.iter_batched_ref(
            || fresh_world::<A>(region),
            |w| map_bulk::<A, S>(w, n_leaves),
            BatchSize::PerIteration,
        );
    });
}

// ---------------------------------------------------------------------------
// Benchmark groups
// ---------------------------------------------------------------------------

fn bench_commit(c: &mut Criterion) {
    let mut g = c.benchmark_group("map/commit_4KiB_one_at_a_time");
    run_commit::<Riscv64Sv39>(&mut g, "Sv39", MIB);
    run_commit::<Riscv64Sv48>(&mut g, "Sv48", MIB);
    run_commit::<Riscv64Sv57>(&mut g, "Sv57", MIB);
    g.finish();
}

fn bench_bulk_4kib(c: &mut Criterion) {
    // 64 MiB region -> 16384 leaves;
    let mut g = c.benchmark_group("map/bulk/4KiB");
    let leaves = 64 * MIB / Size4KiB::BYTES;
    run_bulk::<Riscv64Sv39, Size4KiB>(&mut g, "Sv39", leaves, 8 * MIB);
    run_bulk::<Riscv64Sv48, Size4KiB>(&mut g, "Sv48", leaves, 8 * MIB);
    run_bulk::<Riscv64Sv57, Size4KiB>(&mut g, "Sv57", leaves, 8 * MIB);
    g.finish();
}

fn bench_bulk_2mib(c: &mut Criterion) {
    // 1 GiB region -> 512 leaves
    let mut g = c.benchmark_group("map/bulk/2MiB");
    let leaves = GIB / Size2MiB::BYTES;
    run_bulk::<Riscv64Sv39, Size2MiB>(&mut g, "Sv39", leaves, 2 * MIB);
    run_bulk::<Riscv64Sv48, Size2MiB>(&mut g, "Sv48", leaves, 2 * MIB);
    run_bulk::<Riscv64Sv57, Size2MiB>(&mut g, "Sv57", leaves, 2 * MIB);
    g.finish();
}

fn bench_bulk_1gib(c: &mut Criterion) {
    // 128 GiB region -> 128 leaves.
    let mut g = c.benchmark_group("map/bulk/1GiB");
    let leaves = 128 * GIB / Size1GiB::BYTES;
    run_bulk::<Riscv64Sv39, Size1GiB>(&mut g, "Sv39", leaves, 2 * MIB);
    run_bulk::<Riscv64Sv48, Size1GiB>(&mut g, "Sv48", leaves, 2 * MIB);
    run_bulk::<Riscv64Sv57, Size1GiB>(&mut g, "Sv57", leaves, 2 * MIB);
    g.finish();
}

criterion_group!(
    benches,
    bench_commit,
    bench_bulk_4kib,
    bench_bulk_2mib,
    bench_bulk_1gib
);
criterion_main!(benches);
