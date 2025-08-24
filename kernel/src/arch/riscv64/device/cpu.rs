// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::cell::RefCell;
use core::fmt;
use core::str::FromStr;

use bitflags::bitflags;

use crate::arch::device;
use crate::device_tree::DeviceTree;
use crate::irq::InterruptController;

#[derive(Debug)]
pub struct Cpu {
    pub extensions: RiscvExtensions,
    pub cbop_block_size: Option<usize>,
    pub cboz_block_size: Option<usize>,
    pub cbom_block_size: Option<usize>,
    pub plic: RefCell<device::plic::Plic>,
    pub clock: kasync::time::Clock,
}

bitflags! {
    /// Known RISC-V extensions.
    ///
    /// Note that this is *not* the complete set of all standardized RISC-V extensions, merely the
    /// subset of extensions we care about. If we add features conditional on extensions, we should
    /// add them here.
    #[derive(Debug, Default, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct RiscvExtensions: u16 {
        /// Base ISA
        const I = 1 << 0;
        /// Integer Multiplication and Division
        const M = 1 << 1;
        /// Atomic Instructions
        const A = 1 << 2;
        /// Single-Precision Floating-Point
        const F = 1 << 3;
        /// Double-Precision Floating-Point
        const D = 1 << 4;
        /// Compressed Instructions
        const C = 1 << 5;
        /// Control and Status Register Access
        const ZICSR = 1 << 6;
        /// Basic Performance Counters
        const ZICNTR = 1 << 7;
        /// Main memory supports instruction fetch with atomicity requirement
        ///
        /// Main memory regions with both the cacheability and coherence PMAs must support
        /// instruction fetch, and any instruction fetches of naturally aligned power-of-2 sizes up to
        /// min(ILEN,XLEN) (i.e., 32 bits for RVA20) are atomic.
        const ZICCIF = 1 << 8;
        /// Main memory supports forward progress on LR/SC sequences.
        ///
        /// Main memory regions with both the cacheability and coherence PMAs must support
        /// RsrvEventual.
        const ZICCRSE = 1 << 9;
        /// Main memory supports all atomics in A.
        ///
        /// Main memory regions with both the cacheability and coherence PMAs must support
        /// AMOArithmetic.
        const ZICCAMOA = 1 << 10;
        /// Reservation set size of at most 128 bytes.
        ///
        /// Reservation sets must be contiguous, naturally aligned, and at most 128 bytes in size.
        const ZA128RS = 1 << 11;
        /// Main memory supports misaligned loads/stores.
        ///
        /// Misaligned loads and stores to main memory regions with both the cacheability and
        /// coherence PMAs must be supported.
        const ZICCLSM = 1 << 12;

        /// Hardware Performance Counters
        const ZIHPM = 1 << 13;
    }
}

impl fmt::Display for Cpu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{:<17} : {}", "RISCV EXTENSIONS", self.extensions)?;
        writeln!(f, "{:<17} : {:?}", "CBOP BLOCK SIZE", self.cbop_block_size)?;
        writeln!(f, "{:<17} : {:?}", "CBOZ BLOCK SIZE", self.cboz_block_size)?;
        writeln!(f, "{:<17} : {:?}", "CBOM BLOCK SIZE", self.cbom_block_size)?;
        writeln!(f, "{:<17} : {:?}", "PLIC", self.plic)?;
        writeln!(f, "{:<17} : {}", "CLOCK", self.clock)?;

        Ok(())
    }
}

impl Cpu {
    pub fn interrupt_controller(&self) -> core::cell::RefMut<'_, dyn InterruptController> {
        core::cell::RefMut::map(self.plic.borrow_mut(), |plic| {
            plic as &mut dyn InterruptController
        })
    }

    pub fn new(devtree: &DeviceTree, cpuid: usize) -> crate::Result<Self> {
        let cpus = devtree
            .find_by_path("/cpus")
            .expect("required /cpus node not in device tree");

        let cpu = cpus
            .children()
            .find(|dev| {
                let name = dev.name.name;
                let unit_addr =
                    usize::from_str(dev.name.unit_address.expect("CPU is missing unit address"))
                        .expect("CPU unit address is not an integer");

                name == "cpu" && unit_addr == cpuid
            })
            .expect("CPU node not found in device tree");

        let cbop_block_size = cpu
            .property("riscv,cbop-block-size")
            .map(|prop| prop.as_usize().unwrap());

        let cboz_block_size = cpu
            .property("riscv,cboz-block-size")
            .map(|prop| prop.as_usize().unwrap());

        let cbom_block_size = cpu
            .property("riscv,cbom-block-size")
            .map(|prop| prop.as_usize().unwrap());

        let extensions = cpu.property("riscv,isa-extensions").unwrap().as_strlist()?;
        let extensions = parse_riscv_extensions(extensions);

        // TODO find CLINT associated with this core
        let hlic_node = cpu
            .children()
            .find(|c| c.name.name == "interrupt-controller")
            .unwrap();
        tracing::trace!("CPU interrupt controller: {:?}", hlic_node);

        let mut plic = device::plic::Plic::new(devtree, hlic_node)?;
        plic.irq_unmask(10);

        let clock = device::clock::new(cpu)?;

        Ok(Self {
            clock,
            extensions,
            cbop_block_size,
            cboz_block_size,
            cbom_block_size,
            plic: RefCell::new(plic),
        })
    }

    pub const fn supports_rva20u64(&self) -> bool {
        const RVA20U64: RiscvExtensions = RiscvExtensions::from_bits_retain(
            RiscvExtensions::I.bits()
                | RiscvExtensions::M.bits()
                | RiscvExtensions::A.bits()
                | RiscvExtensions::F.bits()
                | RiscvExtensions::D.bits()
                | RiscvExtensions::C.bits()
                | RiscvExtensions::ZICSR.bits()
                | RiscvExtensions::ZICNTR.bits()
                | RiscvExtensions::ZICCIF.bits()
                | RiscvExtensions::ZICCRSE.bits()
                | RiscvExtensions::ZICCAMOA.bits()
                | RiscvExtensions::ZA128RS.bits()
                | RiscvExtensions::ZICCLSM.bits(),
        );

        self.extensions.contains(RVA20U64)
    }
}

fn parse_riscv_extensions(strs: fdt::StringList) -> RiscvExtensions {
    let mut out = RiscvExtensions::empty();

    for str in strs {
        out |= match str {
            "i" => RiscvExtensions::I,
            "m" => RiscvExtensions::M,
            "a" => RiscvExtensions::A,
            "f" => RiscvExtensions::F,
            "d" => RiscvExtensions::D,
            "c" => RiscvExtensions::C,
            "zicsr" => RiscvExtensions::ZICSR,
            "zicntr" => RiscvExtensions::ZICNTR,
            "ziccif" => RiscvExtensions::ZICCIF,
            "ziccrse" => RiscvExtensions::ZICCRSE,
            "ziccamoa" => RiscvExtensions::ZICCAMOA,
            "za128rs" => RiscvExtensions::ZA128RS,
            "zicclsm" => RiscvExtensions::ZICCLSM,
            "zihpm" => RiscvExtensions::ZIHPM,
            ext => {
                tracing::trace!("unknown RISCV extension {ext}");
                continue;
            }
        }
    }

    out
}

impl fmt::Display for RiscvExtensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        bitflags::parser::to_writer(self, f)
    }
}
