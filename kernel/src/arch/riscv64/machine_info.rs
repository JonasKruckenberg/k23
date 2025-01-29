// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::machine_info::{NodeAtlas, NodeId};
use bitflags::bitflags;
use fdt::Node;

#[derive(Debug)]
pub struct HartLocalMachineInfo {
    pub extensions: RiscvExtensions,
    pub cbop_block_size: Option<usize>,
    pub cboz_block_size: Option<usize>,
    pub cbom_block_size: Option<usize>,
    pub hlic_phandle: u32,
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

pub fn parse_hart_local(atlas: &NodeAtlas, id: &NodeId, node: &Node) -> HartLocalMachineInfo {
    let cbop_block_size = node
        .property("riscv,cbop-block-size")
        .map(|prop| prop.as_usize().unwrap());

    let cboz_block_size = node
        .property("riscv,cboz-block-size")
        .map(|prop| prop.as_usize().unwrap());

    let cbom_block_size = node
        .property("riscv,cbom-block-size")
        .map(|prop| prop.as_usize().unwrap());

    let extensions = node
        .property("riscv,isa-extensions")
        .unwrap()
        .as_strlist()
        .unwrap();
    let extensions = parse_riscv_extensions(extensions).unwrap();

    let hlic = atlas
        .get(&id.append(atlas, "/interrupt-controller").unwrap())
        .unwrap();
    let compatible = hlic.property("compatible").unwrap().as_str().unwrap();
    assert!(
        compatible.contains("riscv,cpu-intc"),
        "compatible ({compatible}) is not a valid RISCV HLIC"
    );
    let hlic_phandle = hlic.property("phandle").unwrap().as_u32().unwrap();

    HartLocalMachineInfo {
        extensions,
        cbop_block_size,
        cboz_block_size,
        cbom_block_size,
        hlic_phandle,
    }
}

pub fn parse_riscv_extensions(strs: fdt::StringList) -> Result<RiscvExtensions, dtb_parser::Error> {
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
            _ => {
                log::error!("unknown RISCV extension {str}");
                // TODO better error type
                return Err(dtb_parser::Error::InvalidToken(0));
            }
        }
    }

    Ok(out)
}
