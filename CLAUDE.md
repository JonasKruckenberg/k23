# k23

k23 is an experimental WebAssembly microkernel OS written in Rust, targeting RISC-V 64-bit.

## Key Documentation

- **[Code Style Guide](manual/src/contributing/style-guide.md)** — conventions not enforced by tooling: error handling, unsafe patterns, async rules, crate organization, no_std requirements.
- **[Architecture Deep Dive](manual/src/contributing/architecture.md)** — detailed internals: memory management layers, async executor, WASM VM pipeline, trap handling, testing infrastructure.
- **[Architecture Overview](manual/src/overview.md)** — high-level: bootloader, kernel, WASM runtime.
- **[System Startup](manual/src/startup.md)** — two-stage boot flow and kernel startup phases.

## Quick Reference

- Build and run: `just build && just run`
- Full validation: `just preflight`
- Hosted tests: `just test`
- On-target (QEMU): `just test-riscv64`
- Concurrency tests: `just loom`
- Lint: `just clippy`
