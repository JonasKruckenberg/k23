use alloc::vec::Vec;
use core::ffi::CStr;
use core::fmt;
use core::fmt::Formatter;
use core::ops::Range;
use dtb_parser::{DevTree, Node, Strings, Visitor};
use pmm::PhysicalAddress;

/// Information about the machine we're running on.
/// This is collected from the FDT (flatting device tree) passed to us by the previous stage loader.
pub struct MachineInfo<'dt> {
    /// The FDT blob passed to us by the previous stage loader
    pub fdt: &'dt [u8],
    /// The boot arguments passed to us by the previous stage loader.
    pub bootargs: Option<&'dt CStr>,
    /// The RNG seed passed to us by the previous stage loader.
    pub rng_seed: Option<&'dt [u8]>,
    pub rtc: Option<Range<PhysicalAddress>>,
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
        if let Some(rtc) = &self.rtc {
            writeln!(
                f,
                "{:<22} : {}..{}",
                "REAL-TIME CLOCK DEVICE", rtc.start, rtc.end
            )?;
        } else {
            writeln!(f, "{:<22} : None", "REAL-TIME CLOCK DEVICE")?;
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
            rtc: v.soc_visitor.rtc.regs.regs.first().cloned(),
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
    soc_visitor: SocVisitor,
}

impl<'dt> Visitor<'dt> for MachineInfoVisitor<'dt> {
    type Error = dtb_parser::Error;
    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        if name.is_empty() {
            node.visit(self)?;
        } else if name == "chosen" {
            node.visit(&mut self.chosen_visitor)?;
        } else if name == "soc" {
            node.visit(&mut self.soc_visitor)?;
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

#[derive(Default, Debug)]
struct SocVisitor {
    rtc: RtcVisitor,
}
impl<'dt> Visitor<'dt> for SocVisitor {
    type Error = dtb_parser::Error;

    fn visit_address_cells(&mut self, size_in_cells: u32) -> Result<(), Self::Error> {
        let size_in_bytes = size_in_cells as usize * size_of::<u32>();

        self.rtc.regs.address_size = size_in_bytes;

        Ok(())
    }

    fn visit_size_cells(&mut self, size_in_cells: u32) -> Result<(), Self::Error> {
        let size_in_bytes = size_in_cells as usize * size_of::<u32>();

        self.rtc.regs.width_size = size_in_bytes;

        Ok(())
    }

    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        if name.starts_with("rtc@") {
            node.visit(&mut self.rtc)?;
        }
        Ok(())
    }
}

#[derive(Default, Debug)]
struct RtcVisitor {
    regs: RegsVisitor,
}

impl<'dt> Visitor<'dt> for RtcVisitor {
    type Error = dtb_parser::Error;
    fn visit_reg(&mut self, reg: &'dt [u8]) -> Result<(), Self::Error> {
        self.regs.visit_reg(reg)
    }
    fn visit_compatible(&mut self, mut strings: Strings<'dt>) -> Result<(), Self::Error> {
        let s = strings.next()?.unwrap();
        assert_eq!(s, "google,goldfish-rtc");
        Ok(())
    }
}

#[derive(Default, Debug)]
struct RegsVisitor {
    address_size: usize,
    width_size: usize,
    regs: Vec<Range<PhysicalAddress>>,
}

impl<'dt> Visitor<'dt> for RegsVisitor {
    type Error = dtb_parser::Error;

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

            self.regs.push(start..start.add(width));
        }

        Ok(())
    }
}
