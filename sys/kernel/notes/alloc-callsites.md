# Kernel allocation callsite inventory

Working document for the per-arena heap migration. Scope: `sys/kernel/src/**`.
Third-party crate allocations (cranelift, wasmparser, hashbrown, ouroboros,
anyhow, futures, kasync, etc.) are **out of grep scope** and must be measured
via runtime instrumentation (`alloc_trace`).

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
| `arch/riscv64/asid_allocator.rs:16,52` | `OnceLock<u16>`; `vec![0; bitmap_size]` (dyn, sized once) |
| `tracing/mod.rs:34,54,80` | `Arc<Subscriber>` |
| `tracing/log.rs:146-150` | 5× `LazyLock<Fields>` |
| `wasm/code_registry.rs:16` | `OnceLock<RwLock<GlobalRegistry>>` |
| `shell.rs:39` | `OnceLock<Barrier>` |

## B. Per-frame-list-node (★★★)

Hot path. Uniform fixed size. Inserted into wavltree per page-range.

| Site | Type |
|---|---|
| `mem/frame_alloc/frame_list.rs:123,141,274,512,534` | `Box::pin(FrameListNode)` |

## C. Per-VMO / per-region (★★★)

Uniform sizes, intrusive-tree linked.

| Site | Type |
|---|---|
| `mem/address_space.rs:412,476` | `Box::pin(AddressSpaceRegion)` (wavltree) |
| `mem/address_space_region.rs:59,77` | `Arc<Vmo>` (zeroed / phys) |
| `mem/vmo.rs:37` | `Vmo::Paged(RwLock<PagedVmo { frames: FrameList }>)` |
| `mem/address_space.rs:729` | `actions: vec![]` (dyn, per `MmapAction`, transient) |

## D. Per-async-task (★★)

| Site | Notes |
|---|---|
| `shell.rs:47`, `tests/mod.rs:127` | `Executor::try_spawn` — task box lives in `kasync` |
| `arch/riscv64/block_on.rs:101` | `Arc<HartNotify>` per CPU-local |
| `irq.rs:38,68` | `HashMap<irq, Arc<WaitQueue>>` |

Investigate `kasync` task allocation separately; likely already pooled.

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

## F. WASM compile pipeline (★★★, bump arena)

`wasm/compile/**`, `wasm/cranelift/**`, `wasm/translate/**`. ~29 indirect
allocs + ~117 grow-ops by grep, plus everything cranelift/wasmparser allocate
internally. Lifetime = single compile job. Best fit: bumpalo arena per job
(crate is already a dep), drop at job end.

10 concurrent compiles per `main.rs:78` — dominant volume during selftest.

## G. Dynamic-capacity Vecs (special handling, not fixed-size slabs)

Capacity depends on input, not on type. Three strategies:
1. Bump arena per scope (compile job, mmap call, trap) — best for transient.
2. Power-of-two size-class slabs (Linux `kmalloc-N`) — fallback.
3. Reusable per-CPU buffers — clear-and-reuse for hot transients.

| Site | Capacity source | Lifetime | Strategy |
|---|---|---|---|
| `mem/frame_alloc/mod.rs:75` `arenas` | # boot regions | static after init | leave |
| `mem/address_space.rs:729` `actions` | mmap op count | per call | (3) per-CPU buf |
| `arch/riscv64/mem.rs:202,217` `wired_frames` | grows w/ pgtables | per AddressSpace | (1) aspace arena |
| `arch/riscv64/asid_allocator.rs:52` bitmap | `bitmap_size` | static | leave |
| `wasm/instance.rs:68` exports | `exports().len()` | per instance | size class |
| `wasm/type_registry.rs:582,643,845` | type counts | per module | size class |
| `wasm/types.rs:1482,1490` params/results | sig arity | per signature | (3) per-CPU buf |
| `wasm/translate/type_convert.rs:105,106` | param/result counts | transient | (1) job arena |
| `wasm/cranelift/env.rs:1483,1750` `real_call_args` | call_args + 2 | per IR call | (3) per-CPU buf |
| `wasm/cranelift/code_translator.rs:386,428` | branch target count | per opcode | (1) job arena |
| `wasm/trap_handler.rs:507` `frames` | unwind depth | per trap | (3) per-CPU buf |
| `wasm/cranelift/builtins.rs:124,125` | param/return | per builtin | (1) job arena |

## H. Trap-path

| Site | Allocation |
|---|---|
| `arch/riscv64/trap_handler.rs:391,423` | `Box::new(payload)` |

Trap allocations should not depend on a healthy heap. Consider moving to a
pre-reserved bump slab.

## I. Out of grep scope (need runtime tracer)

These will dominate volume during the WASM compile selftest:

- `cranelift_codegen` / `cranelift_frontend` IR + regalloc
- `wasmparser` parsing
- `hashbrown` rehashes inside our `HashMap`s
- `tracing` event allocations
- `kasync` task allocations
- `FrameList::from_iter` (`mem/frame_alloc/mod.rs:164`, `provider.rs:83`) → one node per frame

## Quick-win arena candidates

1. `FrameListNode` (B) — uniform, hot, intrusive-tree.
2. `AddressSpaceRegion` (C) — uniform, intrusive-tree.
3. WASM compile job (F) — bump arena, drop at end.
4. Hot per-CPU reusable buffers for (G) entries marked (3).
