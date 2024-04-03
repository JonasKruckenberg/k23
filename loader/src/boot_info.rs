use arrayvec::ArrayVec;
use core::mem;
use core::ops::Range;
use core::ptr::NonNull;
use dtb_parser::{DevTree, Node, Visitor};
use vmm::PhysicalAddress;

#[allow(dead_code)]
#[derive(Debug)]
pub struct BootInfo {
    /// The number of "standalone" CPUs in the system
    pub cpus: usize,
    /// Address ranges we may use for allocation
    pub memories: ArrayVec<Range<PhysicalAddress>, 16>,
}

impl BootInfo {
    pub fn from_dtb(dtb_ptr: NonNull<u8>) -> Self {
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
struct BootInfoVisitor {
    cpus: usize,
    memories: RegsVisitor,
}

#[derive(Default)]
struct RegsVisitor {
    address_size: usize,
    width_size: usize,
    regs: ArrayVec<Range<PhysicalAddress>, 16>,
}

impl BootInfoVisitor {
    pub fn result(self) -> BootInfo {
        BootInfo {
            cpus: self.cpus,
            memories: self.memories.regs,
        }
    }
}

impl<'dt> Visitor<'dt> for BootInfoVisitor {
    type Error = dtb_parser::Error;
    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        if name == "cpus" || name == "" {
            node.visit(self)?;
        } else if name.starts_with("cpu@") {
            self.cpus += 1;
        } else if name.starts_with("memory@") {
            node.visit(&mut self.memories)?;
        }

        Ok(())
    }

    fn visit_address_cells(&mut self, size_in_cells: u32) -> Result<(), Self::Error> {
        let size_in_bytes = size_in_cells as usize * mem::size_of::<u32>();

        self.memories.address_size = size_in_bytes;

        Ok(())
    }

    fn visit_size_cells(&mut self, size_in_cells: u32) -> Result<(), Self::Error> {
        let size_in_bytes = size_in_cells as usize * mem::size_of::<u32>();

        self.memories.width_size = size_in_bytes;

        Ok(())
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
