use crate::error::Error;
use core::mem;
use core::ops::Range;
use dtb_parser::{Dtb, Node, Visit};

#[derive(Debug)]
pub struct BoardInfo {
    pub cpus: usize,
    pub base_frequency: u32,
    pub serial: Serial,
    pub clint: Range<usize>,
    pub qemu_test: Option<Range<usize>>,
}

#[derive(Debug)]
pub struct Serial {
    pub mmio_regs: Range<usize>,
    pub clock_frequency: u32,
}

impl BoardInfo {
    pub fn from_raw(dtb_ptr: *const u8) -> crate::Result<Self> {
        let dtb = unsafe { Dtb::from_raw(dtb_ptr) }.unwrap();

        let mut visitor = BoardInfoVisitor::default();
        dtb.walk(&mut visitor)?;

        Ok(Self {
            cpus: visitor.cpus_visitor.cpus,
            base_frequency: visitor
                .cpus_visitor
                .base_frequency
                .ok_or(Error::MissingBordInfo("base_frequency"))?,
            serial: Serial {
                mmio_regs: visitor
                    .soc_visitor
                    .serial_visitor
                    .regs
                    .inner
                    .ok_or(Error::MissingBordInfo("serial.regs"))?,
                clock_frequency: visitor
                    .soc_visitor
                    .serial_visitor
                    .clock_frequency
                    .ok_or(Error::MissingBordInfo("serial.clock_frequency"))?,
            },
            clint: visitor
                .soc_visitor
                .clint
                .ok_or(Error::MissingBordInfo("clint"))?,
            qemu_test: visitor.soc_visitor.qemu_test,
        })
    }
}

#[derive(Default)]
struct BoardInfoVisitor {
    pub cpus_visitor: CpusVisitor,
    pub soc_visitor: SocVisitor,
}

#[derive(Default)]
struct CpusVisitor {
    pub cpus: usize,
    pub base_frequency: Option<u32>,
}

#[derive(Default)]
struct SocVisitor {
    pub serial_visitor: SerialVisitor,
    pub clint: Option<Range<usize>>,
    pub qemu_test: Option<Range<usize>>,
    addr_size: usize,
    width_size: usize,
}

#[derive(Default)]
struct SerialVisitor {
    pub regs: RegsVisitor,
    pub clock_frequency: Option<u32>,
}

#[derive(Default)]
struct RegsVisitor {
    pub inner: Option<Range<usize>>,
    addr_size: usize,
    width_size: usize,
}

impl<'a> Visit<'a> for BoardInfoVisitor {
    fn visit_subnode(&mut self, name: &'a str, node: Node<'a>) -> Result<(), dtb_parser::Error> {
        if name == "cpus" {
            node.walk(&mut self.cpus_visitor)?;
        } else if name == "soc" {
            node.walk(&mut self.soc_visitor)?;
        }
        Ok(())
    }
}

impl<'a> Visit<'a> for CpusVisitor {
    fn visit_subnode(&mut self, name: &'a str, _node: Node<'a>) -> Result<(), dtb_parser::Error> {
        if name.starts_with("cpu@") {
            self.cpus += 1;
        }
        Ok(())
    }

    fn visit_property(&mut self, name: &'a str, value: &'a [u8]) -> Result<(), dtb_parser::Error> {
        if name == "timebase-frequency" {
            self.base_frequency = Some(u32::from_be_bytes(value.try_into().unwrap()));
        }
        Ok(())
    }
}

impl<'a> Visit<'a> for SocVisitor {
    fn visit_subnode(
        &mut self,
        name: &'a str,
        mut node: Node<'a>,
    ) -> Result<(), dtb_parser::Error> {
        if name.starts_with("serial@") {
            self.serial_visitor.regs.addr_size = self.addr_size;
            self.serial_visitor.regs.width_size = self.width_size;
            node.walk(&mut self.serial_visitor)?;
        } else if name.starts_with("clint@") {
            let mut clint_visitor = RegsVisitor::new(self.addr_size, self.width_size);
            node.walk(&mut clint_visitor)?;
            self.clint = clint_visitor.inner;
        } else if name.starts_with("test@") {
            let mut qemu_test_visitor = RegsVisitor::new(self.addr_size, self.width_size);
            node.walk(&mut qemu_test_visitor)?;
            self.qemu_test = qemu_test_visitor.inner;
        }
        Ok(())
    }

    fn visit_address_cells(&mut self, cells: u32) -> Result<(), dtb_parser::Error> {
        self.addr_size = cells as usize * mem::size_of::<u32>();
        Ok(())
    }

    fn visit_size_cells(&mut self, cells: u32) -> Result<(), dtb_parser::Error> {
        self.width_size = cells as usize * mem::size_of::<u32>();
        Ok(())
    }
}

impl SerialVisitor {
    pub fn new(addr_size: usize, width_size: usize) -> Self {
        Self {
            regs: RegsVisitor::new(addr_size, width_size),
            clock_frequency: None,
        }
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

impl RegsVisitor {
    pub fn new(addr_size: usize, width_size: usize) -> Self {
        Self {
            inner: None,
            addr_size,
            width_size,
        }
    }
}

impl<'a> Visit<'a> for RegsVisitor {
    fn visit_reg(&mut self, reg: &[u8]) -> Result<(), dtb_parser::Error> {
        let (reg, rest) = reg.split_at(self.addr_size);
        let (width, _) = rest.split_at(self.width_size);

        let reg = usize::from_be_bytes(reg.try_into().unwrap());
        let width = usize::from_be_bytes(width.try_into().unwrap());

        self.inner = Some(reg..reg + width);

        Ok(())
    }
}
