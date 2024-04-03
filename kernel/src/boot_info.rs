use arrayvec::ArrayVec;
use core::mem;
use core::ops::Range;
use dtb_parser::{DevTree, Node, Visitor};
use spin::Once;
use vmm::PhysicalAddress;

pub static BOOT_INFO: Once<BootInfo> = Once::new();

#[derive(Debug)]
pub struct BootInfo {
    /// The number of "standalone" CPUs in the system
    pub cpus: usize,
    /// Address ranges we may use for allocation
    pub memories: ArrayVec<Range<PhysicalAddress>, 16>,
    /// Information about the systems UART device
    pub serial: Serial,
}

/// Information about the systems UART device
#[derive(Debug)]
pub struct Serial {
    /// The MMIO registers reserved for this device
    pub reg: Range<PhysicalAddress>,
    /// The clock frequency configured
    pub clock_frequency: u32,
}

impl BootInfo {
    pub fn from_dtb(dtb_ptr: *const u8) -> Self {
        let fdt = unsafe { DevTree::from_raw(dtb_ptr) }.unwrap();
        let mut reservations = fdt.reserved_entries();

        let mut v = BootInfoVisitor::default();
        fdt.visit(&mut v).unwrap();

        let mut info = v.result();

        // Take reserved areas into account
        while let Some(entry) = reservations.next_entry().unwrap() {
            let entry = unsafe {
                let start = PhysicalAddress::new(entry.address as usize);

                start..start.add(entry.size as usize)
            };

            let memories = info.memories.take();

            for mut region in memories {
                if entry.contains(&region.start) && entry.contains(&region.end) {
                    // remove region
                    continue;
                } else if region.contains(&entry.start) && region.contains(&entry.end) {
                    info.memories.push(region.start..entry.start);
                    info.memories.push(entry.end..region.end);
                } else if entry.contains(&region.start) {
                    region.start = entry.end;
                    info.memories.push(region);
                } else if entry.contains(&region.end) {
                    region.end = entry.start;
                    info.memories.push(region);
                }
            }
        }

        info
    }
}

/*
----------------------------------------------------------------------------------------------------
    visitors
----------------------------------------------------------------------------------------------------
*/

#[derive(Default)]
struct BootInfoVisitor<'dt> {
    node: Option<dtb_parser::Node<'dt>>,
    /// Stack of encountered `#address-cells` values, used to correctly read `reg` properties.
    ///
    /// The fixed upper bound of 8 elements is just a guess, technically FDTs can have arbitrary depth.
    /// But we don't have an allocator and 8 seems to be a reasonable choice in practice.
    address_sizes: ArrayVec<usize, 8>,
    /// Stack of encountered `#size-cells` values, used to correctly read `reg` properties.
    width_sizes: ArrayVec<usize, 8>,

    cpus: usize,
    memories: ArrayVec<Range<PhysicalAddress>, 16>,
    serial: Option<Serial>,
}

struct SerialVisitor {
    pub regs: RegsVisitor,
    pub clock_frequency: Option<u32>,
}

struct RegsVisitor {
    address_size: usize,
    width_size: usize,
    regs: ArrayVec<Range<PhysicalAddress>, 16>,
}

impl<'dt> BootInfoVisitor<'dt> {
    pub fn result(self) -> BootInfo {
        BootInfo {
            cpus: self.cpus,
            memories: self.memories,
            serial: self.serial.unwrap(),
        }
    }

    fn regs_visitor(&self) -> RegsVisitor {
        RegsVisitor {
            regs: ArrayVec::new(),
            address_size: *self.address_sizes.last().unwrap(),
            width_size: *self.width_sizes.last().unwrap(),
        }
    }
}

impl<'dt> Visitor<'dt> for BootInfoVisitor<'dt> {
    type Error = dtb_parser::Error;
    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        self.node = Some(node.clone());

        if name.starts_with("cpu@") {
            self.cpus += 1;
        } else if name.starts_with("memory@") {
            let mut v = self.regs_visitor();
            node.visit(&mut v)?;
            self.memories = v.result();
        } else {
            let addr_len = self.address_sizes.len();
            let width_len = self.width_sizes.len();

            node.visit(self)?;

            self.address_sizes.truncate(addr_len);
            self.width_sizes.truncate(width_len);
        }

        Ok(())
    }

    fn visit_address_cells(&mut self, size_in_cells: u32) -> Result<(), Self::Error> {
        let size_in_bytes = size_in_cells as usize * mem::size_of::<u32>();

        self.address_sizes.push(size_in_bytes);

        Ok(())
    }

    fn visit_size_cells(&mut self, size_in_cells: u32) -> Result<(), Self::Error> {
        let size_in_bytes = size_in_cells as usize * mem::size_of::<u32>();

        self.width_sizes.push(size_in_bytes);

        Ok(())
    }

    fn visit_compatible(
        &mut self,
        mut strings: dtb_parser::Strings<'dt>,
    ) -> core::result::Result<(), Self::Error> {
        while let Some(str) = strings.next()? {
            if str == "ns16550a" {
                if let Some(node) = self.node.take() {
                    let mut v = SerialVisitor {
                        regs: self.regs_visitor(),
                        clock_frequency: None,
                    };
                    node.visit(&mut v)?;
                    self.serial = v.result();
                }
            }
        }

        Ok(())
    }
}

impl<'dt> Visitor<'dt> for SerialVisitor {
    type Error = dtb_parser::Error;

    fn visit_reg(&mut self, reg: &'dt [u8]) -> Result<(), Self::Error> {
        self.regs.visit_reg(reg)
    }

    fn visit_property(&mut self, name: &'dt str, value: &'dt [u8]) -> Result<(), Self::Error> {
        if name == "clock-frequency" {
            self.clock_frequency = Some(u32::from_be_bytes(value.try_into().unwrap()));
        }

        Ok(())
    }
}

impl SerialVisitor {
    pub fn result(self) -> Option<Serial> {
        Some(Serial {
            reg: self.regs.result().into_iter().next().unwrap(),
            clock_frequency: self.clock_frequency?,
        })
    }
}

impl<'dt> Visitor<'dt> for RegsVisitor {
    type Error = dtb_parser::Error;

    fn visit_reg(&mut self, mut reg: &'dt [u8]) -> Result<(), Self::Error> {
        while !reg.is_empty() {
            debug_assert_ne!(self.address_size, 0);
            debug_assert_ne!(self.width_size, 0);

            let (start, rest) = reg.split_at(self.address_size);
            let (width, rest) = rest.split_at(self.width_size);
            reg = rest;

            let start = usize::from_be_bytes(start.try_into().unwrap());
            let width = usize::from_be_bytes(width.try_into().unwrap());

            // Safety: start is read from the FDT
            let start = unsafe { PhysicalAddress::new(start) };

            self.regs.push(start..start.add(width));
        }

        Ok(())
    }
}

impl RegsVisitor {
    pub fn result(self) -> ArrayVec<Range<PhysicalAddress>, 16> {
        self.regs
    }
}
