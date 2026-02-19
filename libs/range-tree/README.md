# `range-tree`

A fast [B+ Tree] implementation storing *non-overlapping* *ranges* of integers.

The API is similar to the standard library's [BTreeMap] with some significant differences:

- Instead of associating a *keys* with a values, `RangeTree` associates *ranges* of *integers* with value.
- Queries return the *containing* range-value pair.
- Range indices must be integer types or convertible to integers via the `RangeTreeIndex` trait.
- The maximum integer value is reserved for internal use and cannot be used by ranges.
- Ranges must be **non-overlapping**
- Ranges in the tree are ordered by their integer values instead of their (possible) `Ord` implementation.
- Iterators only support forward iteration.
- Provides a builtin, efficient way to iterate over all gaps between ranges.

This crate is based on [BrieTree] crate by Amanieu d'Antras (licensed under Apache-2.0 OR MIT), which is based on
the [B- Tree] by Sergey Slotin with extensive modifications.

[B+ Tree]: https://en.wikipedia.org/wiki/B%2B_tree
[`BTreeMap`]: https://doc.rust-lang.org/std/collections/struct.BTreeMap.html
[B- Tree]: https://en.algorithmica.org/hpc/data-structures/b-tree/
[BrieTree]: https://github.com/Amanieu/brie-tree

## SIMD

Searching *within* B-Tree nodes is done using an efficient SIMD search. Allowing
lookups to be much faster than the standard library (or most other search trees).

Currently, SIMD optimizations are implemented for the following targets:

| SIMD instruction set   | Target feature flags to use             |
|------------------------|-----------------------------------------|
| x86 SSE2 (x86-64-v1)   | `+sse2` (enabled by default on x86-64)  |
| x86 SSE4.2 (x86-64-v2) | `+sse4.2,+popcnt`                       |
| x86 AVX2 (x86-64-v3)   | `+avx2,+popcnt`                         |
| x86 AVX512 (x86-64-v4) | `+avx512bw,+popcnt`                     |
| AArch64 NEON           | `+neon` (enabled by default on AArch64) |
| AArch64 SVE            | `+sve`                                  |
| RISC-V RVV             | `+v`                                    |

In the absence of these, there is a fallback implementation using an
unrolled binary search.

For the time being, this crate doesn't implement dynamic dispatch since
its performance overhead would likely be prohibitive, Instead, the
target features must be enabled at compile-time by passing them to
`-C target-feature=`:

```sh
RUSTFLAGS="-C target-feature=+avx2,+popcnt" cargo build --release
```