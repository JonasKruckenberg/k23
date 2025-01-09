<div align="center">
  <h1>
    <code>k23</code>
  </h1>
  <p>
    <strong>Experimental WASM Microkernel Operating System</strong>
  </p>
  <p>
  <a href="https://jonaskruckenberg.github.io/k23/">Manual</a>
  <a href="https://discord.gg/KUGGcUS5cW">k23 Discord</a>

[![MIT licensed][mit-badge]][mit-url]

  </p>
</div>

[mit-badge]: https://img.shields.io/badge/license-MIT-blue.svg
[mit-url]: LICENSE

## About

k23 is an active research project exploring a *more secure, modular, and easy to develop for* operating system by using WebAssembly as the primary execution environment.

## Roadmap

- **Phase 0 - Bringup**
   - [x] Bootloader stage
   - [x] Risc-V Support
   - [x] Cryptographic Kernel Integrity
   - [x] Compressed Kernel Support
   - [x] Test Runner & Testing against WASM specification
   - [x] Kernel Backtraces
   - [x] Proper Error Handling
   - [x] KASLR (Kernel Address Space Layout Randomization)
- **Phase 1 - Basic WASM Features**
   - [x] Guest memory management
   - [x] Guest ASLR (Address Space Layout Randomization)
   - [ ] Executing WASM
   - [x] Handling Guest Traps & Fault Recovery
   - [ ] WASM module Imports & Exports
   - [ ] Execute WASM in Userspace
   - [ ] Support WASM Builtins
   - [ ] Handle WASM Traps
   - [ ] Syscall context switching & Basic Host Functions 
   - [ ] WASM Proposal - Extended Constant Expressions
   - [ ] WASM Proposal - Multi-Value
   - [ ] WASM Proposal - Tail Call
   - [ ] WASM Proposal - Reference Types
   - [ ] WASM Proposal - Fixed-width SIMD
   - [ ] WASM Proposal - Relaxed SIMD
   - [ ] WASM Proposal - Multiple Memories
- **Phase 2 - Concurrency**
   - [ ] Kernel Concurrency
   - [ ] Scheduler
   - [ ] WASM Proposal - Threads (Atomics)
   - [ ] WASM Proposal - Shared Everything Threads
- **Phase 2.5 - Kotlin on k23**
   - [ ] WASM Proposal - Garbage Collection
   - [ ] WASM Proposal - Exception Handling
- **Phase 3 - Drivers**
   - [ ] Support MMIO regions (WASM Memory Control Proposal *or* Typed Multiple Memories)
   - [ ] WASM Proposal - Component Model
   - [ ] WASM Component Linking

## Contributing

I believe OS development should be fun, easy, and approachable. If you would like to hack on k23, fork it for your own experiments or just hang out and philosophize about computers. Be my guest! You can [join our small, but growing community][discord-url] of likeminded, awesome people that all believe better computer system are possible!

[discord-url]: https://discord.gg/KUGGcUS5cW
