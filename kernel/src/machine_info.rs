use crate::arch;
use crate::arch::RiscvExtensions;
use alloc::vec::Vec;
use core::ffi::CStr;
use core::fmt::Formatter;
use core::range::Range;
use core::{fmt, mem};
use dtb_parser::{DevTree, Node, Strings, Visitor};
use mmu::PhysicalAddress;

/// Information about the machine we're running on.
/// This is collected from the FDT (flatting device tree) passed to us by the previous stage loader.
pub struct MachineInfo<'dt> {
    /// The FDT blob passed to us by the previous stage loader
    pub fdt: &'dt [u8],
    /// The boot arguments passed to us by the previous stage loader.
    pub bootargs: Option<&'dt CStr>,
    /// The RNG seed passed to us by the previous stage loader.
    pub rng_seed: Option<&'dt [u8]>,
    /// MMIO devices
    pub mmio_devices: Vec<MmioDevice<'dt>>,
}

pub struct MmioDevice<'dt> {
    pub regions: Vec<Range<PhysicalAddress>>,
    pub name: &'dt str,
    pub compatible: Vec<&'dt str>,
}

impl fmt::Debug for MachineInfo<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("MachineInfo")
            .field("fdt", &self.fdt.as_ptr_range())
            .field("bootargs", &self.bootargs)
            .field("rng_seed", &self.rng_seed)
            .finish()
    }
}

impl fmt::Display for MachineInfo<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{:<22} : {:?}",
            "DEVICE TREE BLOB",
            self.fdt.as_ptr_range()
        )?;
        if let Some(bootargs) = self.bootargs {
            writeln!(f, "{:<22} : {:?}", "BOOTARGS", bootargs)?;
        } else {
            writeln!(f, "{:<22} : None", "BOOTARGS")?;
        }
        if let Some(rng_seed) = self.rng_seed {
            writeln!(f, "{:<22} : {:?}", "PRNG SEED", rng_seed)?;
        } else {
            writeln!(f, "{:<22} : None", "PRNG SEED")?;
        }
        for (idx, r) in self.mmio_devices.iter().enumerate() {
            for range in &r.regions {
                writeln!(
                    f,
                    "MMIO DEVICE {:<11}: {}..{} {:<20} {:?}",
                    idx, range.start, range.end, r.name, r.compatible
                )?;
            }
        }

        Ok(())
    }
}

impl MachineInfo<'_> {
    /// Parse the FDT blob and extract the machine information.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the `dtb_ptr` points to a valid FDT blob.
    pub unsafe fn from_dtb(dtb_ptr: *const u8) -> crate::Result<Self> {
        let fdt = unsafe { DevTree::from_raw(dtb_ptr) }?;
        let fdt_slice = fdt.as_slice();

        let mut v = MachineInfoVisitor::default();
        fdt.visit(&mut v)?;

        Ok(MachineInfo {
            fdt: fdt_slice,
            bootargs: v.chosen.bootargs,
            rng_seed: v.chosen.rng_seed,
            mmio_devices: v.soc.regions,
        })
    }
}

pub struct HartLocalMachineInfo {
    /// The hartid of the current hart.
    pub hartid: usize,
    /// Timebase frequency in Hertz for the Hart.
    pub timebase_frequency: usize,
    pub riscv_extensions: RiscvExtensions,
    pub riscv_cbop_block_size: Option<usize>,
    pub riscv_cboz_block_size: Option<usize>,
    pub riscv_cbom_block_size: Option<usize>,
}

impl fmt::Display for HartLocalMachineInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "{:<22} : {}", "HARTID", self.hartid)?;
        writeln!(
            f,
            "{:<22} : {}",
            "TIMEBASE FREQUENCY", self.timebase_frequency
        )?;
        writeln!(
            f,
            "{:<22} : {:?}",
            "RISCV EXTENSIONS", self.riscv_extensions
        )?;
        if let Some(size) = self.riscv_cbop_block_size {
            writeln!(f, "{:<22} : {}", "CBOP BLOCK SIZE", size)?;
        } else {
            writeln!(f, "{:<22} : None", "CBOP BLOCK SIZE")?;
        }
        if let Some(size) = self.riscv_cboz_block_size {
            writeln!(f, "{:<22} : {}", "CBOZ BLOCK SIZE", size)?;
        } else {
            writeln!(f, "{:<22} : None", "CBOZ BLOCK SIZE")?;
        }
        if let Some(size) = self.riscv_cbom_block_size {
            writeln!(f, "{:<22} : {}", "CBOM BLOCK SIZE", size)?;
        } else {
            writeln!(f, "{:<22} : None", "CBOM BLOCK SIZE")?;
        }

        Ok(())
    }
}

impl HartLocalMachineInfo {
    /// Parse the FDT blob and extract the machine information.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the `dtb_ptr` points to a valid FDT blob.
    pub unsafe fn from_dtb(hartid: usize, dtb_ptr: *const u8) -> crate::Result<Self> {
        let fdt = unsafe { DevTree::from_raw(dtb_ptr) }?;

        let mut v = HartLocalMachineInfoVisitor::default();
        fdt.visit(&mut v)?;

        Ok(Self {
            hartid,
            timebase_frequency: v.cpus.timebase_frequency,
            riscv_extensions: v.cpus.riscv_extensions,
            riscv_cbop_block_size: v.cpus.riscv_cbop_block_size,
            riscv_cboz_block_size: v.cpus.riscv_cboz_block_size,
            riscv_cbom_block_size: v.cpus.riscv_cbom_block_size,
        })
    }
}

/*--------------------------------------------------------------------------------------------------
    visitors
---------------------------------------------------------------------------------------------------*/
#[derive(Default)]
struct MachineInfoVisitor<'dt> {
    chosen: ChosenVisitor<'dt>,
    soc: SocVisitor<'dt>,
}

impl<'dt> Visitor<'dt> for MachineInfoVisitor<'dt> {
    type Error = dtb_parser::Error;
    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        if name.is_empty() {
            node.visit(self)?;
        } else if name == "chosen" {
            node.visit(&mut self.chosen)?;
        } else if name == "soc" {
            node.visit(&mut self.soc)?;
        }

        Ok(())
    }
}

#[derive(Default)]
struct ChosenVisitor<'dt> {
    bootargs: Option<&'dt CStr>,
    rng_seed: Option<&'dt [u8]>,
}

impl<'dt> Visitor<'dt> for ChosenVisitor<'dt> {
    type Error = dtb_parser::Error;

    fn visit_property(&mut self, name: &'dt str, value: &'dt [u8]) -> Result<(), Self::Error> {
        match name {
            "bootargs" => self.bootargs = Some(CStr::from_bytes_until_nul(value)?),
            "rng-seed" => self.rng_seed = Some(value),
            _ => log::warn!("unknown /chosen property: {name}"),
        }

        Ok(())
    }
}

#[derive(Default)]
struct HartLocalMachineInfoVisitor {
    cpus: CpusVisitor,
}
impl<'dt> Visitor<'dt> for HartLocalMachineInfoVisitor {
    type Error = dtb_parser::Error;
    fn visit_subnode(&mut self, name: &str, node: Node<'dt>) -> Result<(), Self::Error> {
        if name.is_empty() {
            node.visit(self)?;
        } else if name == "cpus" {
            node.visit(&mut self.cpus)?;
        }

        Ok(())
    }
}

#[derive(Default)]
struct CpusVisitor {
    hartid: usize,
    default_timebase_frequency: Option<usize>,
    timebase_frequency: usize,
    riscv_extensions: RiscvExtensions,
    riscv_cbop_block_size: Option<usize>,
    riscv_cboz_block_size: Option<usize>,
    riscv_cbom_block_size: Option<usize>,
}

impl CpusVisitor {
    fn cpu_visitor(&self) -> CpuVisitor {
        CpuVisitor::default()
    }
}

impl<'dt> Visitor<'dt> for CpusVisitor {
    type Error = dtb_parser::Error;

    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        if let Some((_, str)) = name.split_once("cpu@") {
            let hartid: usize = str.parse().expect("invalid hartid");

            if hartid == self.hartid {
                let mut v = self.cpu_visitor();
                node.visit(&mut v)?;
                self.timebase_frequency = v
                    .timebase_frequency
                    .or(self.default_timebase_frequency)
                    .expect("RISC-V system with no 'timebase-frequency' in FDT");
                self.riscv_extensions = v.riscv_extensions;
                self.riscv_cbop_block_size = v.riscv_cbop_block_size;
                self.riscv_cboz_block_size = v.riscv_cboz_block_size;
                self.riscv_cbom_block_size = v.riscv_cbom_block_size;
            }
        }

        Ok(())
    }

    fn visit_property(&mut self, name: &'dt str, value: &'dt [u8]) -> Result<(), Self::Error> {
        if name == "timebase-frequency" {
            // timebase-frequency can either be 32 or 64 bits
            // https://devicetree-specification.readthedocs.io/en/latest/chapter3-devicenodes.html#cpus-cpu-node-properties
            let value = match value.len() {
                4 => usize::try_from(u32::from_be_bytes(value.try_into()?))?,
                8 => usize::try_from(u64::from_be_bytes(value.try_into()?))?,
                _ => unreachable!(),
            };
            self.default_timebase_frequency = Some(value);
        }

        Ok(())
    }
}

#[derive(Default)]
struct CpuVisitor {
    timebase_frequency: Option<usize>,
    // TODO maybe move arch-specific info into an arch-specific module
    riscv_extensions: RiscvExtensions,
    riscv_cbop_block_size: Option<usize>,
    riscv_cboz_block_size: Option<usize>,
    riscv_cbom_block_size: Option<usize>,
}

impl<'dt> Visitor<'dt> for CpuVisitor {
    type Error = dtb_parser::Error;

    fn visit_property(&mut self, name: &'dt str, value: &'dt [u8]) -> Result<(), Self::Error> {
        if name == "timebase-frequency" {
            // timebase-frequency can either be 32 or 64 bits
            // https://devicetree-specification.readthedocs.io/en/latest/chapter3-devicenodes.html#cpus-cpu-node-properties
            let value = match value.len() {
                4 => usize::try_from(u32::from_be_bytes(value.try_into()?))?,
                8 => usize::try_from(u64::from_be_bytes(value.try_into()?))?,
                _ => unreachable!(),
            };
            self.timebase_frequency = Some(value);
        } else if name == "riscv,isa-extensions" {
            self.riscv_extensions = arch::parse_riscv_extensions(Strings::new(value))?;
        } else if name == "riscv,cbop-block-size" {
            let value = match value.len() {
                4 => usize::try_from(u32::from_be_bytes(value.try_into()?))?,
                8 => usize::try_from(u64::from_be_bytes(value.try_into()?))?,
                _ => unreachable!(),
            };
            self.riscv_cbop_block_size = Some(value);
        } else if name == "riscv,cboz-block-size" {
            let value = match value.len() {
                4 => usize::try_from(u32::from_be_bytes(value.try_into()?))?,
                8 => usize::try_from(u64::from_be_bytes(value.try_into()?))?,
                _ => unreachable!(),
            };
            self.riscv_cboz_block_size = Some(value);
        } else if name == "riscv,cbom-block-size" {
            let value = match value.len() {
                4 => usize::try_from(u32::from_be_bytes(value.try_into()?))?,
                8 => usize::try_from(u64::from_be_bytes(value.try_into()?))?,
                _ => unreachable!(),
            };
            self.riscv_cbom_block_size = Some(value);
        }

        Ok(())
    }
}

#[derive(Default)]
struct SocVisitor<'dt> {
    regions: Vec<MmioDevice<'dt>>,
    address_size: usize,
    width_size: usize,
    child: SocVisitorChildVisitor<'dt>,
}

#[derive(Default)]
struct SocVisitorChildVisitor<'dt> {
    regs: Vec<Range<PhysicalAddress>>,
    address_size: usize,
    width_size: usize,
    name: &'dt str,
    compatible: Vec<&'dt str>,
}

impl<'dt> Visitor<'dt> for SocVisitor<'dt> {
    type Error = dtb_parser::Error;

    fn visit_address_cells(&mut self, size_in_cells: u32) -> Result<(), Self::Error> {
        let size_in_bytes = size_in_cells as usize * size_of::<u32>();

        self.address_size = size_in_bytes;

        Ok(())
    }

    fn visit_size_cells(&mut self, size_in_cells: u32) -> Result<(), Self::Error> {
        let size_in_bytes = size_in_cells as usize * size_of::<u32>();

        self.width_size = size_in_bytes;

        Ok(())
    }

    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        self.child.name = name;
        self.child.address_size = self.address_size;
        self.child.width_size = self.width_size;
        node.visit(&mut self.child)?;
        self.regions.push(self.child.result());
        Ok(())
    }
}

impl<'dt> SocVisitorChildVisitor<'dt> {
    fn result(&mut self) -> MmioDevice<'dt> {
        MmioDevice {
            regions: mem::take(&mut self.regs),
            name: self.name,
            compatible: mem::take(&mut self.compatible),
        }
    }
}

impl<'dt> Visitor<'dt> for SocVisitorChildVisitor<'dt> {
    type Error = dtb_parser::Error;

    fn visit_address_cells(&mut self, size_in_cells: u32) -> Result<(), Self::Error> {
        let size_in_bytes = size_in_cells as usize * size_of::<u32>();

        self.address_size = size_in_bytes;

        Ok(())
    }

    fn visit_size_cells(&mut self, size_in_cells: u32) -> Result<(), Self::Error> {
        let size_in_bytes = size_in_cells as usize * size_of::<u32>();

        self.width_size = size_in_bytes;

        Ok(())
    }

    fn visit_compatible(&mut self, strings: Strings<'dt>) -> Result<(), Self::Error> {
        self.compatible = strings.collect::<Result<_, _>>()?;

        Ok(())
    }

    fn visit_reg(&mut self, mut reg: &'dt [u8]) -> Result<(), Self::Error> {
        debug_assert_ne!(self.address_size, 0);
        debug_assert_ne!(self.width_size, 0);

        while !reg.is_empty() {
            let (start, rest) = reg.split_at(self.address_size);
            let (width, rest) = rest.split_at(self.width_size);
            reg = rest;

            let start = usize::from_be_bytes(start.try_into()?);
            let width = usize::from_be_bytes(width.try_into()?);

            let start = PhysicalAddress::new(start);
            self.regs
                .push(Range::from(start..start.checked_add(width).unwrap()));
        }

        Ok(())
    }
}
