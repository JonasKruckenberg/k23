use core::ffi::CStr;
use core::fmt;
use core::fmt::Formatter;
use dtb_parser::{DevTree, Node, Visitor};

/// Information about the machine we're running on.
/// This is collected from the FDT (flatting device tree) passed to us by the previous stage loader.
pub struct MachineInfo<'dt> {
    /// The FDT blob passed to us by the previous stage loader
    pub fdt: &'dt [u8],
    /// The boot arguments passed to us by the previous stage loader.
    pub bootargs: Option<&'dt CStr>,
    /// The RNG seed passed to us by the previous stage loader.
    pub rng_seed: Option<&'dt [u8]>,
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
            "{:<20} : {:?}",
            "DEVICE TREE BLOB",
            self.fdt.as_ptr_range()
        )?;
        if let Some(bootargs) = self.bootargs {
            writeln!(f, "{:<20} : {:?}", "BOOTARGS", bootargs)?;
        } else {
            writeln!(f, "{:<20} : None", "BOOTARGS")?;
        }
        if let Some(rng_seed) = self.rng_seed {
            writeln!(f, "{:<20} : {:?}", "PRNG SEED", rng_seed)?;
        } else {
            writeln!(f, "{:<20} : None", "PRNG SEED")?;
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
            bootargs: v.chosen_visitor.bootargs,
            rng_seed: v.chosen_visitor.rng_seed,
        })
    }
}

pub struct HartLocalMachineInfo {
    /// The hartid of the current hart.
    pub hartid: usize,
    /// Timebase frequency in Hertz for the Hart.
    pub timebase_frequency: usize,
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
        })
    }
}

/*--------------------------------------------------------------------------------------------------
    visitors
---------------------------------------------------------------------------------------------------*/
#[derive(Default)]
struct MachineInfoVisitor<'dt> {
    chosen_visitor: ChosenVisitor<'dt>,
}

impl<'dt> Visitor<'dt> for MachineInfoVisitor<'dt> {
    type Error = dtb_parser::Error;
    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        if name.is_empty() {
            node.visit(self)?;
        } else if name == "chosen" {
            node.visit(&mut self.chosen_visitor)?;
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
impl Visitor<'_> for HartLocalMachineInfoVisitor {
    type Error = dtb_parser::Error;
    fn visit_subnode(&mut self, name: &str, node: Node) -> Result<(), Self::Error> {
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
                let timebase_frequency = v.result();
                self.timebase_frequency = timebase_frequency
                    .or(self.default_timebase_frequency)
                    .expect("RISC-V system with no 'timebase-frequency' in FDT");
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
}

impl CpuVisitor {
    fn result(self) -> Option<usize> {
        self.timebase_frequency
    }
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
        }

        Ok(())
    }
}
