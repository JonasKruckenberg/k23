# Supported WASM Features & Proposals

This page documents all the WASM features, APIs and proposals that k23 supports. This list will be revised as time
progresses and features are implemented.

## Standardized Features

These features have been adopted into the WebAssembly standard and k23 aims to support all applicable features.

| Features                                                         | Status | Tracking Issue                                           |
|------------------------------------------------------------------|--------|----------------------------------------------------------|
| [JS BigInt to Wasm i64 integration][bigint-to-i64]               | N/A    |                                                          |
| [Bulk memory operations][bulk-memory]                            | ✅      |                                                          |
| [Extended Constant Expressions][extended-const]                  | ❌      | [#31](https://github.com/JonasKruckenberg/k23/issues/31) |
| [Garbage collection][garbage_collection]                         | ❌      | [#32](https://github.com/JonasKruckenberg/k23/issues/32) |
| [Multiple memories][multi-memory]                                | ❌      | [#33](https://github.com/JonasKruckenberg/k23/issues/33) |
| [Multi-value][multi-value]                                       | ❌      | [#34](https://github.com/JonasKruckenberg/k23/issues/34) |
| [Mutable globals][mutable-global]                                | ✅      |                                                          |
| [Reference types][reference-types]                               | ❌      | [#35](https://github.com/JonasKruckenberg/k23/issues/35) |
| [Relaxed SIMD][relaxed-simd]                                     | ❌      | [#36](https://github.com/JonasKruckenberg/k23/issues/36) |
| [Non-trapping float-to-int conversions][saturating-float-to-int] | ✅      |                                                          |
| [Sign-extension operations][sign-extension]                      | ✅      |                                                          |
| [Fixed-width SIMD][simd]                                         | ❌      | [#37](https://github.com/JonasKruckenberg/k23/issues/37) |
| [Tail call][tail_call]                                           | ❌      | [#38](https://github.com/JonasKruckenberg/k23/issues/38) |
| [Threads][threads]                                               | ❌      | [#39](https://github.com/JonasKruckenberg/k23/issues/39) |

## Proposals

These features are proposals for the WebAssembly standard.
Many proposals change very frequently and support for them will range from limited to non-existent.
Additionally some proposals may not be applicable to k23.

| Features                                                     | Status | Tracking Issue                                           |
|--------------------------------------------------------------|--------|----------------------------------------------------------|
| [Typed Function References][function_references]             | ❌      | [#44](https://github.com/JonasKruckenberg/k23/issues/44) |
| [Custom Annotation Syntax in the Text Format][annotations]   | ❌      | [#45](https://github.com/JonasKruckenberg/k23/issues/45) |
| [Branch Hinting][branch-hinting]                             | ❌      | [#43](https://github.com/JonasKruckenberg/k23/issues/43) |
| [Exception handling][exception_handling]                     | ❌      | [#42](https://github.com/JonasKruckenberg/k23/issues/42) |
| [Memory64][memory64]                                         | ❌      | [#41](https://github.com/JonasKruckenberg/k23/issues/41) |
| [Web Content Security Policy][content-security-policy]       | N/A    |
| [JS Promise Integration][js-promise-integration]             | N/A    |
| [Type Reflection for WebAssembly JavaScript API][js-types]   | N/A    |
| [ESM Integration][ecmascript_module_integration]             | N/A    |
| [JS String Builtins][js-string-builtins]                     | N/A    |
| [Relaxed dead code validation][relaxed-dead-code-validation] | ❌      | [#46](https://github.com/JonasKruckenberg/k23/issues/46) |
| [Numeric Values in WAT Data Segments][numeric-values-in-wat] | ❌      | [#47](https://github.com/JonasKruckenberg/k23/issues/47) |
| [Instrument and Tracing Technology][instrument-tracing]      | ?      | [#48](https://github.com/JonasKruckenberg/k23/issues/48) |
| [Extended Name Section][extended-name-section]               | ❌      | [#40](https://github.com/JonasKruckenberg/k23/issues/40) |
| [Type Imports][type-imports]                                 | ❌      |
| [Component Model][component-model]                           | ❌      |
| [WebAssembly C and C++ API][wasm_c_api]                      | N/A    |
| [Flexible Vectors][flexible-vectors]                         | ?      |
| [Call Tags][call-tags]                                       | ❌      |
| [Stack Switching][stack-switching]                           | ❌      |
| [Constant Time][constant-time]                               | ❌      | [#50](https://github.com/JonasKruckenberg/k23/issues/50) |
| [JS Customization for GC Objects][gc-js-customization]       | N/A    |
| [Memory control][memory-control]                             | ❌      |
| [Reference-Typed Strings][stringref]                         | N/A    |
| [Profiles][profiles]                                         | ?      |
| [Rounding Variants][rounding-mode-control]                   | ❌      |
| [Shared-Everything Threads][shared-everything-threads]       | ❌      |
| [Frozen Values][frozen-values]                               | ?      |
| [Compilation Hints][compilation-hints]                       | ❌      | [#49](https://github.com/JonasKruckenberg/k23/issues/49) |
| [Custom Page Sizes][custom-page-sizes]                       | ❌      | [#51](https://github.com/JonasKruckenberg/k23/issues/51) |
| [Half Precision][half-precision]                             | ❌      |
| [Compact Import Section][compact-import-section]             | ?      |

Explainer

- ✅: Implemented
- ❌: Not Implemented
- ?: The applicability of this feature is unclear, e.g. due to the lack of a detailed proposal.
- N/A: Not Applicable

## WASI Features & Proposals

In addition to the main WASM features, k23 applications will interact with the host through WASI (WebAssembly System
Interface) APIs.
The following table lists all current proposals and their implementation status.

| Features                               | Status      | Tracking Issue |
|----------------------------------------|-------------|----------------|
| [I/O][wasi_io]                         | ❌           |
| [Clocks][wasi_clocks]                  | ❌           |
| [Random][wasi_random]                  | ❌           |
| [Filesystem][wasi_fs]                  | ❌           |
| [Sockets][wasi_sockets]                | ❌           |
| [CLI][wasi_cli]                        | ❌           |
| [HTTP][wasi_http]                      | ❌           |
| [Machine Learning][wasi_nn]            | ❌           |
| [Clocks: Timezone][wasi_timezone]      | X           |
| [Blob Store][wasi_blobs]               | Not planned |
| [Crypto][wasi_crypto]                  | ❌           |
| [Digital I/O][wasi_digital_io]         | ?           |
| [Distributed Lock Service][wasi_locks] | Not planned |
| [I2C][wasi_i2c]                        | ❌           |
| [Key-value Store][wasi_kv]             | ❌           |
| [Logging][wasi_log]                    | ❌           |
| [Messaging][wasi_msg]                  | ❌           |
| [Observe][wasi_observe]                | ❌           |
| [Parallel][wasi_parallel]              | ❌           |
| [Pattern Match][wasi_pattern_match]    | ?           |
| [Runtime Config][wasi_cfg]             | ?           |
| [SPI][wasi_spi]                        | ?           |
| [SQL][wasi_sql]                        | ?           |
| [SQL Embed][wasi_sql_embed]            | N/A         |
| [Threads][wasi_threads]                | Not Planned |
| [URL][wasi_url]                        | ?           |
| [USB][wasi_USB]                        | ❌           |
| [WebGPU][wasi_webgpu]                  | ❌           |

Explainer

- ✅: Implemented
- ❌: Not Implemented
- ?: The applicability of this feature is unclear, e.g. due to the lack of a detailed proposal.
- N/A: Not Applicable

[bigint-to-i64]: https://github.com/WebAssembly/JS-BigInt-integration

[bulk-memory]: https://github.com/WebAssembly/bulk-memory-operations/blob/master/proposals/bulk-memory-operations/Overview.md

[multi-value]: https://github.com/WebAssembly/spec/blob/master/proposals/multi-value/Overview.md

[mutable-global]: https://github.com/WebAssembly/mutable-global/blob/master/proposals/mutable-global/Overview.md

[reference-types]: https://github.com/WebAssembly/reference-types/blob/master/proposals/reference-types/Overview.md

[saturating-float-to-int]: https://github.com/WebAssembly/spec/blob/master/proposals/nontrapping-float-to-int-conversion/Overview.md

[sign-extension]: https://github.com/WebAssembly/spec/blob/master/proposals/sign-extension-ops/Overview.md

[simd]: https://github.com/WebAssembly/simd/blob/master/proposals/simd/SIMD.md

[annotations]: https://github.com/WebAssembly/annotations

[ecmascript_module_integration]: https://github.com/WebAssembly/esm-integration

[exception_handling]: https://github.com/WebAssembly/exception-handling

[feature_detection]: https://github.com/WebAssembly/feature-detection

[function_references]: https://github.com/WebAssembly/function-references

[type-imports]: https://github.com/WebAssembly/proposal-type-imports

[garbage_collection]: https://github.com/WebAssembly/gc

[component-model]: https://github.com/WebAssembly/component-model

[multi-memory]: https://github.com/WebAssembly/multi-memory

[tail_call]: https://github.com/WebAssembly/tail-call

[threads]: https://github.com/webassembly/threads

[js-types]: https://github.com/WebAssembly/js-types

[wasm_c_api]: https://github.com/WebAssembly/wasm-c-api

[content-security-policy]: https://github.com/WebAssembly/content-security-policy

[webassembly_specification]: https://github.com/WebAssembly/spec

[extended-name-section]: https://github.com/WebAssembly/extended-name-section

[constant-time]: https://github.com/WebAssembly/constant-time

[memory64]: https://github.com/WebAssembly/memory64

[flexible-vectors]: https://github.com/WebAssembly/flexible-vectors

[numeric-values-in-wat]: https://github.com/WebAssembly/wat-numeric-values

[instrument-tracing]: https://github.com/WebAssembly/instrument-tracing

[call-tags]: https://github.com/WebAssembly/call-tags

[relaxed-dead-code-validation]: https://github.com/WebAssembly/relaxed-dead-code-validation

[branch-hinting]: https://github.com/WebAssembly/branch-hinting

[extended-const]: https://github.com/WebAssembly/extended-const

[relaxed-simd]: https://github.com/WebAssembly/relaxed-simd

[stack-switching]: https://github.com/WebAssembly/stack-switching

[js-promise-integration]: https://github.com/WebAssembly/js-promise-integration

[gc-js-customization]: https://github.com/WebAssembly/gc-js-customization

[memory-control]: https://github.com/WebAssembly/memory-control

[stringref]: https://github.com/WebAssembly/stringref

[profiles]: https://github.com/WebAssembly/profiles

[js-string-builtins]: https://github.com/WebAssembly/js-string-builtins

[rounding-mode-control]: https://github.com/WebAssembly/rounding-mode-control

[shared-everything-threads]: https://github.com/WebAssembly/shared-everything-threads

[frozen-values]: https://github.com/WebAssembly/frozen-values

[compilation-hints]: https://github.com/WebAssembly/compilation-hints

[custom-page-sizes]: https://github.com/WebAssembly/custom-page-sizes

[half-precision]: https://github.com/WebAssembly/half-precision

[compact-import-section]: https://github.com/WebAssembly/compact-import-section

[wasi_io]: https://github.com/WebAssembly/wasi-io

[wasi_clocks]: https://github.com/WebAssembly/wasi-clocks

[wasi_random]: https://github.com/WebAssembly/wasi-random

[wasi_fs]: https://github.com/WebAssembly/wasi-filesystem

[wasi_sockets]: https://github.com/WebAssembly/wasi-sockets

[wasi_cli]: https://github.com/WebAssembly/wasi-cli

[wasi_http]: https://github.com/WebAssembly/wasi-http

[wasi_nn]: https://github.com/WebAssembly/wasi-nn

[wasi_timezone]: https://github.com/WebAssembly/wasi-clocks

[wasi_blobs]: https://github.com/WebAssembly/wasi-blob-store

[wasi_crypto]: https://github.com/WebAssembly/wasi-crypto

[wasi_digital_io]: https://github.com/WebAssembly/wasi-digital-io

[wasi_locks]: https://github.com/WebAssembly/wasi-distributed-lock-service

[wasi_i2c]: https://github.com/WebAssembly/wasi-i2c

[wasi_kv]: https://github.com/WebAssembly/wasi-kv-store

[wasi_log]: https://github.com/WebAssembly/wasi-logging

[wasi_msg]: https://github.com/WebAssembly/wasi-messaging

[wasi_observe]: https://github.com/dylibso/wasi-observe

[wasi_parallel]: https://github.com/WebAssembly/wasi-parallel

[wasi_pattern_match]: https://github.com/WebAssembly/wasi-pattern-match

[wasi_cfg]: https://github.com/WebAssembly/wasi-runtime-config

[wasi_spi]: https://github.com/WebAssembly/wasi-spi

[wasi_sql]: https://github.com/WebAssembly/wasi-sql

[wasi_sql_embed]: https://github.com/WebAssembly/wasi-sql-embed

[wasi_threads]: https://github.com/WebAssembly/wasi-native-threads

[wasi_url]: https://github.com/WebAssembly/wasi-url

[wasi_USB]: https://github.com/WebAssembly/wasi-usb

[wasi_webgpu]: https://github.com/WebAssembly/wasi-webgpu
