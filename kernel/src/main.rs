// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_std]
#![no_main]
#![feature(allocator_api)]

extern crate alloc;

mod cpu_local;
mod cpu_set;
mod task;
mod utils;

use crate::cpu_set::LogicalCpuId;
use loader_api::BootInfo;
use rand_chacha::ChaCha20Rng;
use sync::Mutex;

#[unsafe(no_mangle)]
pub extern "C" fn _start(cpuid: LogicalCpuId, boot_info: &'static BootInfo, boot_ticks: u64) -> ! {
    todo!()
}

fn kmain(cpuid: LogicalCpuId, boot_info: &'static BootInfo, boot_ticks: u64) {
    // TODO BOOTSTRAP
    //      => [ARCH PER_CPU]
    //          => enable counters                                      needs:
    //          => Set the FPU state to initial                         needs:
    //      => [GLOBAL GLOBAL] init EARLY tracing subscriber            needs:
    //      => [GLOBAL GLOBAL] init boot_alloc                          needs: boot_info
    //      => [GLOBAL GLOBAL] init allocator                           needs: boot_alloc
    //      => [GLOBAL GLOBAL] init backtrace                           needs: boot_info
    //      => [GLOBAL GLOBAL] find FDT                                 needs: boot_info
    //      => [GLOBAL GLOBAL] parse devicetree                         needs: alloc, FDT
    //      => [GLOBAL GLOBAL] init rng                                 needs: boot_info
    //      => [GLOBAL PER_CPU] init boot time                          needs: boot_ticks
    //      => [GLOBAL PER_CPU] init CPU ID                             needs: cpuid
    //      => [ARCH PER_CPU]
    //          => probe SBI extensions                                 needs:
    //          => parse CPU info                                       needs:
    //              => parse RISCV ISA extensions                       needs: devicetree
    //              => parse cbop/cboz/cbom sizes                       needs: devicetree
    //              => init clock                                       needs: devicetree
    //              => init plic                                        needs: devicetree, kernel_aspace
    //      => [GLOBAL PER_CPU] init cpu_local_frame_cache              needs:
    //      => [GLOBAL GLOBAL] init frame_alloc                         needs: bootalloc, FDT
    //      => [ARCH GLOBAL] init arch_aspace                           needs:
    //          => [ARCH GLOBAL] zero lower kernel aspace half          needs:
    //      => [GLOBAL GLOBAL] init kernel_aspace                       needs: frame_alloc, arch_aspace, rng
    //          => [GLOBAL GLOBAL] reserve physmap region               needs: kernel_aspace
    //          => [GLOBAL GLOBAL] reserve kernel ELF regions           needs: kernel_aspace
    //      => [GLOBAL GLOBAL] init WASM engine                         needs:
    //          => [GLOBAL GLOBAL] init JIT compiler                    needs:
    //      => [GLOBAL GLOBAL] init all_tasks                           needs:
    //      => [GLOBAL GLOBAL] init scheduler                           needs: alloc
    //      => [GLOBAL PER_CPU] init scheduler CpuLocal data            needs:
    //      => [GLOBAL PER_CPU] init scheduler worker                   needs: CPUID, rng
    //      => [GLOBAL GLOBAL] parse bootargs                           needs: devicetree
    //      => [GLOBAL GLOBAL] init tracing subscriber                  needs: alloc, filter

    // RUNTIME NEEDS
    // | component      | global dep                      | cpu_local dep                     |
    // |----------------|---------------------------------|-----------------------------------|
    // | tracing        | subscriber (output)             | CPUID, boot_time, (output indent) |
    // | backtrace      | backtrace_info                  |                                   |
    // | devicetree     |                                 |                                   |
    // | frame_alloc    | global_frame_alloc              | cpu_local_frame_cache             |
    // | kernel_aspace  | frame_alloc, THE_ZERO_FRAME     |                                   |
    // | panic          |                                 | panic_count                       |
    // | scheduler      | scheduler                       | worker, timer                     |
    // | wasm engine    | compiler, type registry, stores |
}

struct Global {
    arch: RiscvGlobal,
    /// Global root RNG
    rng: Mutex<ChaCha20Rng>,
    /// Information required to build backtraces
    backtrace_info: (),
    /// The device tree
    devicetree: (),
    /// Global frame allocator
    frame_alloc: (),
    kernel_aspace: (),
}
struct RiscvGlobal {
    /// The set of SBI extensions supported by the firmware
    sbi_extensions: (),
    /// Address space identifier allocator
    asid_alloc: (),
}

struct CpuLocal {
    arch: RiscvCpuLocal,
    /// The ID of this CPU
    cpuid: LogicalCpuId,
    /// The canonical boot time of this CPU, all absolute time durations are based off this value
    boot_time: (),
    /// CPU-local frame allocator cache
    frame_cache: (),
    /// Clock driver for this CPU's monotonic clock
    clock: (),
    /// CPU-local task scheduler state
    scheduler: (),
    /// Linked list of WASM activations
    activations: (),
}
struct RiscvCpuLocal {
    /// The set of RISCV ISA extensions supported by the CPU
    isa_extensions: (),
    /// The blocksize for Zicbom (Cache-Block Management) operations in bytes
    cbom_block_size: Option<usize>,
    /// The blocksize for Zicbop (Cache-Block Prefetch) operations in bytes
    cbop_block_size: Option<usize>,
    /// The blocksize for Zicboz (Cache-Block Zero) operations in bytes
    cboz_block_size: Option<usize>,
    /// Driver for the RISCV Platform-Local Interrupt Controller
    plic: (),
}
