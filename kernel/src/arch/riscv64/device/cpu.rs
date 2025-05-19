// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::CPUID;
use crate::arch::device;
use crate::device_tree::DeviceTree;
use crate::irq::InterruptController;
use anyhow::{Context, bail};
use async_kit::time::{Clock, NANOS_PER_SEC, Ticks, Timer};
use bitflags::bitflags;
use core::cell::{OnceCell, RefCell};
use core::fmt;
use core::str::FromStr;
use core::time::Duration;
use cpu_local::cpu_local;

cpu_local! {
    static CPU: OnceCell<Cpu> = OnceCell::new();
}

#[derive(Debug)]
pub struct Cpu {
    pub extensions: RiscvExtensions,
    pub cbop_block_size: Option<usize>,
    pub cboz_block_size: Option<usize>,
    pub cbom_block_size: Option<usize>,
    pub plic: RefCell<device::plic::Plic>,
}

bitflags! {
    #[derive(Debug, Default, Copy, Clone, Hash, PartialEq, Eq)]
    pub struct RiscvExtensions: u64 {
        const I = 1 << 0;
        const M = 1 << 1;
        const A = 1 << 2;
        const F = 1 << 3;
        const D = 1 << 4;
        const C = 1 << 5;
        const H = 1 << 6;
        const ZIC64B = 1 << 7;
        const ZICBOM = 1 << 8;
        const ZICBOP = 1 << 9;
        const ZICBOZ = 1 << 10;
        const ZICCAMOA = 1 << 11;
        const ZICCIF = 1 << 12;
        const ZICCLSM = 1 << 13;
        const ZICCRSE = 1 << 14;
        const ZICNTR = 1 << 15;
        const ZICSR = 1 << 16;
        const ZIFENCEI = 1 << 17;
        const ZIHINTNTL = 1 << 18;
        const ZIHINTPAUSE = 1 << 19;
        const ZIHPM = 1 << 20;
        const ZMMUL = 1 << 21;
        const ZA64RS = 1 << 22;
        const ZAAMO = 1 << 23;
        const ZALRSC = 1 << 24;
        const ZAWRS = 1 << 25;
        const ZFA = 1 << 26;
        const ZCA = 1 << 27;
        const ZCD = 1 << 28;
        const ZBA = 1 << 29;
        const ZBB = 1 << 30;
        const ZBC = 1 << 31;
        const ZBS = 1 << 32;
        const SSCCPTR = 1 << 33;
        const SSCOUNTERENW = 1 << 34;
        const SSTC = 1 << 35;
        const SSTVALA = 1 << 36;
        const SSTVECD = 1 << 37;
        const SVADU = 1 << 38;
        const SVVPTC = 1 << 39;
    }
}

impl fmt::Display for Cpu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{:<17} : {}", "RISCV EXTENSIONS", self.extensions)?;
        writeln!(f, "{:<17} : {:?}", "CBOP BLOCK SIZE", self.cbop_block_size)?;
        writeln!(f, "{:<17} : {:?}", "CBOZ BLOCK SIZE", self.cboz_block_size)?;
        writeln!(f, "{:<17} : {:?}", "CBOM BLOCK SIZE", self.cbom_block_size)?;
        writeln!(f, "{:<17} : {:?}", "PLIC", self.plic)?;

        Ok(())
    }
}

pub fn with_cpu<F, R>(f: F) -> R
where
    F: FnOnce(&Cpu) -> R,
{
    CPU.with(|cpu_info| f(cpu_info.get().expect("CPU info not initialized")))
}

pub fn try_with_cpu<F, R>(f: F) -> crate::Result<R>
where
    F: FnOnce(&Cpu) -> R,
{
    CPU.with(|cpu_info| cpu_info.get().context("CPU info not initialized").map(f))
}

#[cold]
pub fn init(devtree: &DeviceTree) -> crate::Result<()> {
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

            name == "cpu" && unit_addr == CPUID.get()
        })
        .expect("CPU node not found in device tree");

    // with the CPU node in hand we can go and initialize the global timer
    // all CPUs will race to init the timer, but only on one CPU will be closure actually be called
    // this CPU won the race and its clock becomes the "global" clock
    crate::time::init(|| {
        let timebase_frequency = cpu
            .property("timebase-frequency")
            .or_else(|| cpu.parent().unwrap().property("timebase-frequency"))
            .unwrap()
            .as_u64()?;

        let tick_duration = Duration::from_nanos(NANOS_PER_SEC / timebase_frequency);
        let clock = Clock::new(tick_duration, || Ticks(riscv::register::time::read64()));

        debug_assert_eq!(
            clock.ticks_to_duration(Ticks(timebase_frequency)),
            Duration::from_secs(1)
        );
        debug_assert_eq!(
            clock.duration_to_ticks(Duration::from_secs(1)).unwrap(),
            Ticks(timebase_frequency)
        );

        Ok(Timer::new(clock))
    })?;

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
    let extensions = parse_riscv_extensions(extensions)?;

    // TODO find CLINT associated with this core
    let hlic_node = cpu
        .children()
        .find(|c| c.name.name == "interrupt-controller")
        .unwrap();
    tracing::trace!("CPU interrupt controller: {:?}", hlic_node);

    let mut plic = device::plic::Plic::new(devtree, hlic_node)?;
    plic.irq_unmask(10);

    CPU.with(|info| {
        let info_ = Cpu {
            extensions,
            cbop_block_size,
            cboz_block_size,
            cbom_block_size,
            plic: RefCell::new(plic),
        };
        tracing::debug!("\n{info_}");

        info.set(info_).unwrap();
    });

    Ok(())
}

// fn init_global_timer(cpu: &crate::device_tree::Device) -> crate::Result<()> {
//     // every CPU will attempt to initialize the global timer, but only one will succeed.
//     // It's clock now has become the "global" clock
//     static TIMER: OnceLock<Timer> = OnceLock::new();
//     let timer = TIMER.get_or_try_init(|| -> crate::Result<Timer> {
//
//     })?;
//     let _ = async_kit::time::set_global_timer(timer);
//     Ok(())
// }

pub fn parse_riscv_extensions(strs: fdt::StringList) -> crate::Result<RiscvExtensions> {
    let mut out = RiscvExtensions::empty();

    for str in strs {
        out |= match str {
            "i" => RiscvExtensions::I,
            "m" => RiscvExtensions::M,
            "a" => RiscvExtensions::A,
            "f" => RiscvExtensions::F,
            "d" => RiscvExtensions::D,
            "c" => RiscvExtensions::C,
            "h" => RiscvExtensions::H,
            "zic64b" => RiscvExtensions::ZIC64B,
            "zicbom" => RiscvExtensions::ZICBOM,
            "zicbop" => RiscvExtensions::ZICBOP,
            "zicboz" => RiscvExtensions::ZICBOZ,
            "ziccamoa" => RiscvExtensions::ZICCAMOA,
            "ziccif" => RiscvExtensions::ZICCIF,
            "zicclsm" => RiscvExtensions::ZICCLSM,
            "ziccrse" => RiscvExtensions::ZICCRSE,
            "zicntr" => RiscvExtensions::ZICNTR,
            "zicsr" => RiscvExtensions::ZICSR,
            "zifencei" => RiscvExtensions::ZIFENCEI,
            "zihintntl" => RiscvExtensions::ZIHINTNTL,
            "zihintpause" => RiscvExtensions::ZIHINTPAUSE,
            "zihpm" => RiscvExtensions::ZIHPM,
            "zmmul" => RiscvExtensions::ZMMUL,
            "za64rs" => RiscvExtensions::ZA64RS,
            "zaamo" => RiscvExtensions::ZAAMO,
            "zalrsc" => RiscvExtensions::ZALRSC,
            "zawrs" => RiscvExtensions::ZAWRS,
            "zfa" => RiscvExtensions::ZFA,
            "zca" => RiscvExtensions::ZCA,
            "zcd" => RiscvExtensions::ZCD,
            "zba" => RiscvExtensions::ZBA,
            "zbb" => RiscvExtensions::ZBB,
            "zbc" => RiscvExtensions::ZBC,
            "zbs" => RiscvExtensions::ZBS,
            "ssccptr" => RiscvExtensions::SSCCPTR,
            "sscounterenw" => RiscvExtensions::SSCOUNTERENW,
            "sstc" => RiscvExtensions::SSTC,
            "sstvala" => RiscvExtensions::SSTVALA,
            "sstvecd" => RiscvExtensions::SSTVECD,
            "svadu" => RiscvExtensions::SVADU,
            "svvptc" => RiscvExtensions::SVVPTC,
            ext => {
                bail!("unknown RISCV extension {}", ext);
            }
        }
    }

    Ok(out)
}

impl fmt::Display for RiscvExtensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        bitflags::parser::to_writer(self, f)
    }
}
