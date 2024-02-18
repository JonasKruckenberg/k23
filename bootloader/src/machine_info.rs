use core::mem;
use core::ops::Range;
use dtb_parser::{Dtb, Node, Visit};
use vmm::PhysicalAddress;

/// Information about the machine we're running on, parsed from the Device Tree Blob (DTB) passed
/// to us by a previous boot stage (U-BOOT)
pub struct MachineInfo {
    pub cpus: usize,
    pub serial: Serial,
    pub clint: Range<PhysicalAddress>,
    pub qemu_test: Option<Range<PhysicalAddress>>,
    pub memory: Range<PhysicalAddress>,
}

#[derive(Debug)]
pub struct Serial {
    pub mmio_regs: Range<PhysicalAddress>,
    pub clock_frequency: u32,
}

impl MachineInfo {
    pub fn from_dtb(dtb_ptr: *const u8) -> Self {
        let dtb = unsafe { Dtb::from_raw(dtb_ptr) }.unwrap();

        let mut visitor = Visitor::default();
        dtb.walk(&mut visitor).expect("Failed to parse device tree");

        Self {
            cpus: visitor.cpus,
            serial: Serial {
                mmio_regs: visitor
                    .soc
                    .serial
                    .regs
                    .inner
                    .expect("Missing DTB property `soc.serial.regs`"),
                clock_frequency: visitor
                    .soc
                    .serial
                    .clock_frequency
                    .expect("Missing DTB property `soc.serial.clock-frequency`"),
            },
            clint: visitor
                .soc
                .clint
                .inner
                .expect("Missing DTB property `soc.clint.regs`"),
            qemu_test: visitor.soc.qemu_test.inner,
            memory: visitor
                .memory
                .inner
                .expect("Missing DTB property `memory.regs"),
        }
    }
}

#[derive(Default)]
struct Visitor {
    pub cpus: usize,
    pub soc: SocVisitor,
    pub memory: RegsVisitor,
}

#[derive(Default)]
struct SocVisitor {
    pub serial: SerialVisitor,
    pub clint: RegsVisitor,
    pub qemu_test: RegsVisitor,
}

#[derive(Default)]
struct SerialVisitor {
    pub regs: RegsVisitor,
    pub clock_frequency: Option<u32>,
}

#[derive(Default)]
struct RegsVisitor {
    pub inner: Option<Range<PhysicalAddress>>,
    addr_size: usize,
    width_size: usize,
}

impl<'a> Visit<'a> for Visitor {
    fn visit_subnode(&mut self, name: &'a str, node: Node<'a>) -> Result<(), dtb_parser::Error> {
        if name == "cpus" {
            node.walk(self)?;
        } else if name.starts_with("cpu@") {
            self.cpus += 1;
        } else if name == "soc" {
            node.walk(&mut self.soc)?;
        } else if name.starts_with("memory@") {
            node.walk(&mut self.memory)?;
        }
        Ok(())
    }

    fn visit_address_cells(&mut self, size_in_cells: u32) -> Result<(), dtb_parser::Error> {
        let size_in_bytes = size_in_cells as usize * mem::size_of::<u32>();
        self.memory.addr_size = size_in_bytes;
        Ok(())
    }

    fn visit_size_cells(&mut self, size_in_cells: u32) -> Result<(), dtb_parser::Error> {
        let size_in_bytes = size_in_cells as usize * mem::size_of::<u32>();
        self.memory.width_size = size_in_bytes;
        Ok(())
    }
}

impl<'a> Visit<'a> for SocVisitor {
    fn visit_subnode(&mut self, name: &'a str, node: Node<'a>) -> Result<(), dtb_parser::Error> {
        if name.starts_with("serial@") {
            node.walk(&mut self.serial)?;
        } else if name.starts_with("clint@") {
            node.walk(&mut self.clint)?;
        } else if name.starts_with("test@") {
            node.walk(&mut self.qemu_test)?;
        }
        Ok(())
    }

    fn visit_address_cells(&mut self, size_in_cells: u32) -> Result<(), dtb_parser::Error> {
        let size_in_bytes = size_in_cells as usize * mem::size_of::<u32>();
        self.serial.regs.addr_size = size_in_bytes;
        self.clint.addr_size = size_in_bytes;
        self.qemu_test.addr_size = size_in_bytes;
        Ok(())
    }

    fn visit_size_cells(&mut self, size_in_cells: u32) -> Result<(), dtb_parser::Error> {
        let size_in_bytes = size_in_cells as usize * mem::size_of::<u32>();
        self.serial.regs.width_size = size_in_bytes;
        self.clint.width_size = size_in_bytes;
        self.qemu_test.width_size = size_in_bytes;
        Ok(())
    }
}

impl<'a> Visit<'a> for SerialVisitor {
    fn visit_reg(&mut self, reg: &'a [u8]) -> Result<(), dtb_parser::Error> {
        self.regs.visit_reg(reg)
    }

    fn visit_property(&mut self, name: &'a str, value: &'a [u8]) -> Result<(), dtb_parser::Error> {
        if name == "clock-frequency" {
            self.clock_frequency = Some(u32::from_be_bytes(value.try_into().unwrap()));
        }

        Ok(())
    }
}

impl<'a> Visit<'a> for RegsVisitor {
    fn visit_reg(&mut self, reg: &[u8]) -> Result<(), dtb_parser::Error> {
        let (reg, rest) = reg.split_at(self.addr_size);
        let (width, _) = rest.split_at(self.width_size);

        let reg = usize::from_be_bytes(reg.try_into().unwrap());
        let width = usize::from_be_bytes(width.try_into().unwrap());

        let start = unsafe { PhysicalAddress::new(reg) };
        let end = start.add(width);
        self.inner = Some(start..end);

        Ok(())
    }
}
