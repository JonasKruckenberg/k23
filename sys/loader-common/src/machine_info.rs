// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::range::Range;

use loader_api::FirmwareTables;
use mem_core::PhysicalAddress;

#[derive(Debug)]
pub struct MachineInfo {
    /// Firmware-reported ID of the boot CPU.
    pub boot_hart_id: usize,
    pub firmware_tables: FirmwareTables,
    /// 32-byte seed for the kernel RNG.
    pub rng_seed: [u8; 32],
    /// Console UART.
    pub uart: Option<DiscoveredUart>,
}

/// A console UART resolved from the FDT, with its register block in *physical*
/// space. The loader maps it before handoff; see [`loader_api::UartInfo`].
#[derive(Debug, Clone, Copy)]
pub struct DiscoveredUart {
    /// Physical range of the UART register block (`reg`).
    pub regs: Range<PhysicalAddress>,
    /// Input clock to the baud-rate generator in Hz (`clock-frequency`).
    pub clock_frequency: u32,
    /// Line speed in baud (`stdout-path` options / `current-speed`, else 115200).
    pub baud_rate: u32,
    /// `log2` of the byte stride between registers (`reg-shift`), 0 when absent.
    pub reg_shift: u32,
    /// Width of each register access in bytes (`reg-io-width`), 1 when absent.
    pub reg_io_width: u32,
    pub irq_num: u32,
}
