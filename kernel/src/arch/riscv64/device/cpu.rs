// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::cell::RefCell;
use core::fmt;
use core::str::FromStr;

use riscv::extensions::RiscvExtensions;

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
        self.extensions.contains(RiscvExtensions::RVA20U64)
    }
}

fn parse_riscv_extensions(strs: fdt::StringList) -> RiscvExtensions {
    let mut out = RiscvExtensions::empty();

    for str in strs {
        if let Ok(ext) = RiscvExtensions::from_str(str) {
            out |= ext;
        } else {
            tracing::trace!("unknown RISCV extension {str}");
        }
    }

    out
}
