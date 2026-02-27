# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.9](https://github.com/JonasKruckenberg/k23/compare/wavltree-v0.0.8...wavltree-v0.0.9) - 2026-02-27

### Other

- range tree ([#641](https://github.com/JonasKruckenberg/k23/pull/641))
- *(deps)* update rust crate libfuzzer-sys to v0.4.12 ([#660](https://github.com/JonasKruckenberg/k23/pull/660))
- *(deps)* update rust crate criterion to 0.8.0
- *(kmem)* AddressSpace API and AddressSpaceRegion tree
- *(wavltree)* add context parameter to `assert_valid` method. ([#554](https://github.com/JonasKruckenberg/k23/pull/554))
- *(deps)* update rust crate criterion to 0.7.0 ([#508](https://github.com/JonasKruckenberg/k23/pull/508))
- rustfmt  items ([#506](https://github.com/JonasKruckenberg/k23/pull/506))

## [0.0.8](https://github.com/JonasKruckenberg/k23/compare/v0.0.7...v0.0.8) - 2025-07-11

### Fixed

- *(deps)* update rust crate libfuzzer-sys to v0.4.10 ([#492](https://github.com/JonasKruckenberg/k23/pull/492))

### Other

- update Rust to 1.90.0-nightly ([#499](https://github.com/JonasKruckenberg/k23/pull/499))
- *(deps)* update rust crate criterion to 0.6.0 ([#421](https://github.com/JonasKruckenberg/k23/pull/421))
- overhaul testing ([#482](https://github.com/JonasKruckenberg/k23/pull/482))
- async executor benchmarks ([#442](https://github.com/JonasKruckenberg/k23/pull/442))

## [0.0.7](https://github.com/JonasKruckenberg/k23/compare/v0.0.6...v0.0.7) - 2025-02-21

### Added

- Rust `2024` edition (#309)

## [0.0.6](https://github.com/JonasKruckenberg/k23/compare/v0.0.5...v0.0.6) - 2025-02-15

### Added

- user address space functionality (#246)
- integrate k23VM Wasm Engine (#224)

### Fixed

- reduce time spent on large allocations (#288)

### Other

- update rand crates ([#271](https://github.com/JonasKruckenberg/k23/pull/271))
- upgrade deps ([#268](https://github.com/JonasKruckenberg/k23/pull/268))

## [0.0.5](https://github.com/JonasKruckenberg/k23/compare/v0.0.4...v0.0.5) - 2025-01-11

### Other

- Rust 2024 edition ready (#222)

## [0.0.4](https://github.com/JonasKruckenberg/k23/compare/v0.0.3...v0.0.4) - 2025-01-09

### Added

- implement `AddressSpace::unmap` (#217)

## [0.0.3](https://github.com/JonasKruckenberg/k23/compare/v0.0.2...v0.0.3) - 2025-01-09

### Added

- VM (finally) (#212)
- kernel virtmem cont (#189)

### Other

- add copyright headers ([#213](https://github.com/JonasKruckenberg/k23/pull/213))

## [0.0.2](https://github.com/JonasKruckenberg/k23/compare/v0.0.1...v0.0.2) - 2024-12-16

### Other

- *(kernel/vm)* allocate virtual memory spot (#165)

## [0.0.1](https://github.com/JonasKruckenberg/k23/compare/v0.0.0...v0.0.1) - 2024-12-12

### Other

- remove ambiguous and broken `cursor`/`cursor_mut` in favor of `root`/`root_mut` methods (#164)
- *(wavltree)* release v0.0.0 (#160)

## [0.0.0](https://github.com/JonasKruckenberg/k23/releases/tag/v0.0.0) - 2024-12-12

### Added

- *(wavltree)* range, and entry APIs (#159)
- WAVL Tree (#147)

### Other

- Update README.md
