# Kernel allocation callsite inventory

Working document for the per-arena heap migration. Scope: `sys/kernel/src/**`.
Third-party crate allocations (cranelift, regalloc2, hashbrown, etc.) are
**out of grep scope** but visible to the runtime tracer in `alloc_trace.rs`.

Categories:
- `★★★` strong arena candidate (uniform size + lifetime + count)
- `★★`  fits an arena scheme but secondary
- `★`   probably leave on global heap or pool inline
- `dyn` capacity depends on input — needs special handling, not a fixed-size slab

## A. One-shot init singletons (★)

Allocate once at boot, never freed. No arena win.

| Site | Allocation |
|---|---|
| `state.rs:19` | `OnceLock<Global>` |
| `mem/frame_alloc/mod.rs:32,75,97` | `FRAME_ALLOC`; `Vec<Arena>` (dyn, init only); `Mutex<GlobalFrameAllocator>` |
| `mem/mod.rs:45,79` | `Arc<Mutex<AddressSpace>>` (kernel aspace) |
| `mem/provider.rs:34,49` | `LazyLock<Arc<TheZeroFrame>>` |
| `mem/address_space_region.rs:90` | `LazyLock<Arc<Vmo::Wired>>` |
| `backtrace/mod.rs:24,78` | `OnceLock<BacktraceInfo>` + symbolize ctx |
| `arch/riscv64/asid_allocator.rs:16,52` | `OnceLock<u16>`; `vec![0; bitmap_size]` (dyn) — **see G, allocates per WastContext in test build, not once** |
| `tracing/mod.rs:34,54,80` | `Arc<Subscriber>` |
| `tracing/log.rs:146-150` | 5× `LazyLock<Fields>` |
| `wasm/code_registry.rs:16` | `OnceLock<RwLock<GlobalRegistry>>` |
| `shell.rs:39` | `OnceLock<Barrier>` |

## B. Per-frame-list-node (★★★)

Hot path. Uniform fixed size. Inserted into wavltree per page-range.

| Site | Type |
|---|---|
| `mem/frame_alloc/frame_list.rs:123,141,274,512,534` | `Box::pin(FrameListNode)` |

Not yet observed in the runtime trace because the WASM selftest doesn't drive
heavy frame allocation; will appear under workloads that build/tear-down
address spaces. Still a strong slab candidate.

## C. Per-VMO / per-region (★★★)

Uniform sizes, intrusive-tree linked.

| Site | Type |
|---|---|
| `mem/address_space.rs:412,476` | `Box::pin(AddressSpaceRegion)` (wavltree) |
| `mem/address_space_region.rs:59,77` | `Arc<Vmo>` (zeroed / phys) |
| `mem/vmo.rs:37` | `Vmo::Paged(RwLock<PagedVmo { frames: FrameList }>)` |
| `mem/address_space.rs:729` | `actions: vec![]` (dyn, per `MmapAction`) |

## D. Per-async-task (★★)

| Site | Notes |
|---|---|
| `shell.rs:47`, `tests/mod.rs:127` | `Executor::try_spawn` → `kasync::TaskRef::new_allocated` (`sys/async/src/task.rs:937`) — `Box::new(Task::new(...))` per spawn |
| `arch/riscv64/block_on.rs:101` | `Arc<HartNotify>` per hart — **see F (per-hart allocs), removable** |
| `irq.rs:38,68` | `HashMap<irq, Arc<WaitQueue>>` |

`kasync` allocates a `Box<Task>` per spawn. Fixed size per concrete future
type — slab per task type, but spawned types vary, so size-class slab fits
better than per-type.

## E. WASM module/engine/instance (★★)

Per-module long-lived. Drop-arena per Engine fits.

| Site | Type |
|---|---|
| `wasm/engine.rs:49,70` | `Arc<EngineInner>` |
| `wasm/module.rs:79,85,198,204` | `Arc<Code>`, `Arc<ModuleInner>`, `Arc<CodeObject>` |
| `wasm/store/mod.rs:47` | `Box<StoreInner>` |
| `wasm/linker.rs:61-63,227` | `Vec<String>`, `HashMap`, `Arc<HostFunc>` |
| `wasm/type_registry.rs:755,839,845` | `Arc<RecGroupEntryInner>`, `Arc<Type>`, `Vec<supertypes>` (dyn) |
| `wasm/instance.rs:68` | `vec![None; module.exports().len()]` (dyn, sized once) |
| `wasm/vm/vmcontext.rs:1089` | `Box<VMArrayCallHostFuncContext>` per host func |
| `wasm/func/host.rs:111`, `func/mod.rs:62` | `Box<dyn Fn>` per host func |
| `wasm/translate/module_translator.rs:949` | `Arc<dwarf_sup>` |

## F. Per-hart lazy allocations (★★ reliability)

Allocations that fire on first hart access. Two patterns:

### F.1 `cpu_local!` macro form with allocating init (one alloc per hart)

| Site | Allocation | Removable? |
|---|---|---|
| `arch/riscv64/block_on.rs:100-104` | `Arc::new(HartNotify { hartid, unparked })` | **Yes.** Replace with `#[thread_local] static HART_NOTIFY: HartNotify`; pass raw `*const HartNotify` through the waker vtable. The static outlives any waker the hart can ever produce, so clone/drop become no-ops and there is no Arc allocation. |

The other macro-form `cpu_local!` sites do not allocate (Cell/OnceCell of
small POD, or BSS-resident byte arrays):
- `state.rs:21` `CPU_LOCAL: OnceCell<CpuLocal>`
- `tracing/mod.rs:36-39` `OUTPUT_INDENT`, `CPUID`
- `arch/riscv64/trap_handler.rs:27-29` `IN_TRAP`, `TRAP_STACK` (BSS array)
- `wasm/trap_handler.rs:171` `ACTIVATION`

### F.2 `cpu_local::collection::CpuLocal<T>` lazy buckets (O(log N) allocs per hart)

The collection-style `CpuLocal<T>` (`lib/cpu-local/src/collection.rs`) lazily
allocates power-of-two buckets the first time each hart accesses it. With N
counters across M harts, that's `M × log₂ M × N` heap allocations — small but
unbounded by counter count, and fired on metric-increment paths.

| Site | Type | Notes |
|---|---|---|
| `metrics.rs:38` (every `counter!()` macro) | `CpuLocal<AtomicU64>` | Hot, many instances. Fix: iterate the `.bss.kcounter.*` link sections at boot and force-allocate all buckets for the actual hart count. |
| `tracing/registry.rs:140` `current_spans` | `CpuLocal<SpanStack>` | One instance. Fix: `with_capacity(num_harts)` at construction, eager-allocate all buckets at boot. |
| `mem/frame_alloc/mod.rs:100` `cpu_local_cache` | `CpuLocal<Cache>` | One instance. Same eager-init fix. |

The hart count is taken from `boot_info.cpu_mask.count_ones()` at boot; no
compile-time `MAX_HARTS` bound is required.

## G. Dynamic-capacity Vecs

| Site | Capacity source | Lifetime | Strategy |
|---|---|---|---|
| `mem/frame_alloc/mod.rs:75` `arenas` | # boot regions | static after init | leave |
| `mem/address_space.rs:729` `actions` | mmap op count | per call | per-CPU buf |
| `arch/riscv64/mem.rs:202,217` `wired_frames` | grows w/ pgtables | per AddressSpace | aspace arena |
| `arch/riscv64/asid_allocator.rs:52` bitmap | `bitmap_size` | **per WastContext (test path)** | reuse, or `[u8; N]` BSS |
| `wasm/instance.rs:68` exports | `exports().len()` | per instance | size class |
| `wasm/type_registry.rs:582,643,845` | type counts | per module | size class |
| `wasm/types.rs:1482,1490` params/results | sig arity | per signature | per-CPU buf |
| `wasm/translate/type_convert.rs:105,106` | param/result counts | transient | job arena |
| `wasm/cranelift/env.rs:1483,1750` `real_call_args` | call_args + 2 | per IR call | per-CPU buf |
| `wasm/cranelift/code_translator.rs:386,428` | branch target count | per opcode | job arena |
| `wasm/trap_handler.rs:507` `frames` | unwind depth | per trap | per-CPU buf |
| `wasm/cranelift/builtins.rs:124,125` | param/return | per builtin | job arena |
| `wasm/compile/mod.rs:216` (collect) | input func count | per compile job | `with_capacity` + job arena |
| `wasm/cranelift/compiler.rs:518` (push) | per-input | per compile job | job arena |

## H. Trap-path

| Site | Allocation |
|---|---|
| `arch/riscv64/trap_handler.rs:391,423` | `Box::new(payload)` |

Trap allocations should not depend on a healthy heap. Move to a
pre-reserved bump slab.

## I. Out of grep scope (runtime tracer findings)

Snapshot from a clean WASM-compile selftest run with backtrace symbolization
killswitched off. Sorted by total bytes allocated.

| Rank | Bytes | Allocs | Source | Lifetime | Strategy |
|---|---|---|---|---|---|
| 1 | 196 KB | 48 | `regalloc2::ion::data_structures::VRegs::push` (doubling chain 320→10240) | per compile job | job arena |
| 2 | 64 KB | 8 | `regalloc2::ion::liveranges::create_pregs_and_vregs` resize (8192 each) | per compile job | job arena |
| 3 | 57 KB | 7 | `cranelift_codegen::machinst::vcode::VRegAllocator::with_capacity` | per compile job | job arena |
| 4 | 52 KB | **367 × 144 B** | `regalloc2::ion::liveranges::add_liverange_to_preg` → `BTreeMap::insert` → `LeafNode::new` | per compile job | **★★★ slab** or absorbed by job arena |
| 5 | 47 KB | 5 | `kernel_tests::wasm::compile::CompileInputs::compile` collect | per compile job | `with_capacity` + arena |
| 6 | 32 KB | 1 | `tracing::Registry::default` → `sharded_slab::Array::new` | static after init | leave |
| 7 | 28 KB | 3 | `bumpalo::Bump::new_chunk` for `device_tree::unflatten_property` | per device-tree parse | bigger initial chunk |
| 8 | 20 KB | 9 | `cranelift_codegen::machinst::vcode::VRegAllocator::with_capacity` (HashMap) | per compile job | job arena |
| 9 | 12 KB | 3 | `wast::core::expr::ExpressionParser::push_instr` | per wast parse | bump arena per parse |
| 10 | 10 KB | 12 | `cranelift_codegen::machinst::vcode::VCode::new` | per compile job | job arena |
| 15-17 | 24 KB | 3×8192 | `arch::riscv64::asid_allocator::AsidAllocator::new` from `WastContext::new_default` | **per test (!)** | **see G — kernel-owned fix** |
| 18 | 6 KB | 1 | `kernel_tests::wasm::cranelift::compiler.rs:518` push doubling | per compile job | job arena |
| 19 | 6 KB | 4 | `kasync::Executor::with_capacity` → `TaskRef::new_stub` → `Box::try_new` | static after init | leave |

### Size-class summary (top, clean run)

```
size=144 align=8   375 allocs  10.3%   (367 are BTree leaves from regalloc2)
size=16  align=4   344 allocs   9.4%
size=8   align=1   282 allocs   7.7%
size=2   align=1   266 allocs   7.3%
size=32  align=4   208 allocs   5.7%
size=1   align=1   140 allocs   3.8%
size=16  align=1   139 allocs   3.8%
size=48  align=4   114 allocs   3.1%
size=56  align=8    96 allocs   2.6%
size=64  align=8    84 allocs   2.3%
size=256 align=4    57 allocs   1.6%
```

## J. Symbolizer (deferred but real)

Killswitched out of the active trace, but the previous run showed the kernel's
own `addr2line` driving ~70% of allocations whenever a backtrace is printed:
many small Vecs in `addr2line::line::Lines::parse`, `addr2line::function::*`,
and `gimli::read::line::LineProgramHeader::clone`. Two options when we revisit:
1. Per-context bump arena scoped to one symbolize call.
2. Pre-build a static symbol table at kernel build time so runtime
   symbolization needs only string slicing.

## Quick-win arena candidates (revised)

Priority order based on the trace:

1. **Per-compile-job bump arena** absorbing regalloc2 + cranelift transients
   (#1, #2, #3, #5, #8, #10, #11, #12, #14, #18, #20). ~370 KB collapsed.
2. **144 B BTree-leaf slab** for #4 (only worth it if (1) doesn't already
   absorb it via the arena; cranelift uses a non-bumpalo allocator for the
   regalloc2 BTreeMap).
3. **`FrameListNode` slab (B)** and **`AddressSpaceRegion` slab (C)** — not
   yet visible in the trace but uniform-size hot intrusive-tree nodes.
4. **Per-CPU scratch buffers** for the entries marked "per-CPU buf" in (G).

See `kernel-allocation-todos.md` for the actionable engineering plan.
