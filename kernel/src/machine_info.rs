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
struct CpusVisitor {
    cpus: usize,
    hart_mask: usize,
}

impl CpusVisitor {
    fn cpu_visitor(&self) -> CpuVisitor {
        CpuVisitor::default()
    }
}

impl<'dt> Visitor<'dt> for CpusVisitor {
    type Error = dtb_parser::Error;

    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        if name.starts_with("cpu@") {
            self.cpus += 1;

            let mut v = self.cpu_visitor();
            node.visit(&mut v)?;
            let (hartid, enabled) = v.result();

            if enabled {
                self.hart_mask |= 1 << hartid;
            }
        }

        Ok(())
    }
}

#[derive(Default)]
struct CpuVisitor<'dt> {
    status: Option<&'dt CStr>,
    hartid: usize,
}

impl CpuVisitor<'_> {
    fn result(self) -> (usize, bool) {
        let enabled = self.status.unwrap() != c"disabled";

        (self.hartid, enabled)
    }
}

impl<'dt> Visitor<'dt> for CpuVisitor<'dt> {
    type Error = dtb_parser::Error;

    fn visit_reg(&mut self, reg: &'dt [u8]) -> Result<(), Self::Error> {
        self.hartid = match reg.len() {
            4 => usize::try_from(u32::from_be_bytes(reg.try_into()?))?,
            8 => usize::try_from(u64::from_be_bytes(reg.try_into()?))?,
            _ => unreachable!(),
        };

        Ok(())
    }

    fn visit_property(&mut self, name: &'dt str, value: &'dt [u8]) -> Result<(), Self::Error> {
        if name == "status" {
            self.status = Some(CStr::from_bytes_until_nul(value)?);
        }

        Ok(())
    }
}
