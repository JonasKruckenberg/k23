# Code Style Guide

This guide covers conventions not enforced by tooling. Clippy, rustfmt, and workspace lint settings handle the mechanical stuff — this covers the decisions you have to make yourself.

## Error Handling

**Prefer typed custom errors.** Define an error enum whenever the caller might reasonably want to match on error variants. Co-locate the error type with the code that produces it — if `foo.rs` is the only place that produces `FooError`, the type lives in `foo.rs`, not in a top-level `error.rs`. A library with exactly one error type may put it in `error.rs`, but we are moving away from central error files.

**Use `anyhow` when:**
- Error conditions are still in flux and the enum would change frequently, or
- The error enum would be unwieldy (many variants, all opaque to callers), or
- No caller is expected to match on the error — it's only going to be logged or propagated.

`anyhow` is primarily for the kernel crate and similar top-level consumers. Library crates should reach for it only as a last resort.

**Context.** Always attach context when propagating errors with `.context("...")` or `.with_context(|| ...)`. The message should describe what the code was *trying to do*, not restate what went wrong (the inner error already says that).

```rust
// Good
archive.open(path).context("opening kernel ELF")?;

// Avoid — just restates the error
archive.open(path).context("failed to open")?;
```

## Fallibility and Panics

**Prefer fallible code.** Use `Option`, `Result`, and `try_*` variants wherever possible. `state::try_global()` over `state::global()`, `checked_add` over `+`, etc.

The kernel should only panic in truly exceptional, unrecoverable situations. During development "fail early, fail loudly" is fine — a `todo!()` or `unwrap()` with a clear comment is acceptable scaffolding. But in code intended for production paths, replace panics with proper error propagation.

The one exception is initialization code: if the kernel cannot initialize a critical subsystem it is reasonable to panic, since there is nothing meaningful to recover to.

## `unsafe`

Unsafe code is inevitable in an OS kernel. The rules:

1. **Every `unsafe` block must have a `reason`.** Use `#[expect(unsafe_code, reason = "...")]` or the `unsafe(reason = "...")` form. The reason should be concise — one sentence explaining *why* the invariant holds here, not a re-statement of what the code does.

2. **Isolate behind safe abstractions.** Unsafe implementation details should be hidden behind a safe public API whenever possible. File-level isolation ("all unsafe lives in `unsafe_impl.rs`") is impractical for a kernel and not required, but the surface area of each unsafe block should be minimal.

3. **Unsafe is not a workaround for borrow checker friction.** If the safe version compiles and is equally correct, write the safe version.

## Async

**If it waits for I/O of any kind, make it `async`.** This is an async-first codebase. Blocking in an async context stalls the entire CPU.

**Never busy-loop** except in rare, explicitly justified circumstances (e.g., very short spinlocks before a lock is expected imminently). Busy loops must have a comment explaining why one is warranted.

**Background operations should yield often.** If an async task does a large amount of work without natural suspension points, insert explicit `yield_now()` calls to give other tasks a chance to run.

## State Access

**CpuLocal vs Global:**
- Use `CpuLocal` for state that is duplicated per-CPU or relates to a per-CPU facility (interrupt controller, timer, FPU state, per-CPU RNG seed).
- Use `Global` for OS-wide state that is logically singular (device tree, frame allocator, WASM engine, executor).

**Prefer `try_global()` / fallible accessors.** Panicking accessors (`global()`) exist for convenience but should only be called from code that runs after the relevant subsystem is fully initialized — and even then, prefer the fallible form where it doesn't add significant noise.

## Crate Organization

**`libs/`** contains k-prefixed, OS-specific library crates. Directory names are short (e.g., `spin`); crate package names carry the `k` prefix (e.g., `kspin`). All crates in `libs/` must be `no_std`. A feature flag may add `std` support but must be **off by default**.

**`crates/`** contains more general or standalone crates that do not carry the `k` prefix and are not inherently OS-specific.

**Create a new crate when:**
- The code can be meaningfully reused in isolation, or
- The code must be shared between separate binaries (e.g., between the loader and the kernel — the `loader-api` crate is the canonical example).

Otherwise, add to an existing crate. Crate proliferation has real costs (compilation time, dependency graph complexity).

## no_std

All `libs/` crates are `no_std` without exception. The `#![no_std]` attribute goes at the top of `lib.rs`. If a crate needs allocations, it uses `extern crate alloc` — never `use std::...` in a library crate. `std`-gated feature flags are permitted but must be off by default.

## Re-exports

Only re-export from the crate root when consumers genuinely need the symbol at that path. Avoid blanket `pub use inner::*;` re-exports that inflate the public API surface and make `rustdoc` harder to navigate.

## Copyright Headers

Every new source file must begin with the standard copyright header:

```rust
// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.
```

## Testing

k23 has three test layers — use the right one:

| Layer | When to use |
|-------|-------------|
| Hosted tests (`cargo test`) | Pure logic with no hardware dependencies — data structures, parsers, pure algorithms |
| `#[ktest]` on-target tests | Code that requires the kernel environment — memory allocators, interrupt handling, WASM execution |
| WAST tests | WASM specification compliance — add a `.wast` file under `tests/` |

`#[ktest]` tests are async by default. Prefer async tests unless the code under test is inherently synchronous.
