use crate::kconfig;
use arrayvec::ArrayVec;
use core::cmp::Ordering;
use core::fmt::Formatter;
use core::ops::Range;
use core::{fmt, mem};
use dtb_parser::{DevTree, Node, Visitor};
use vmm::{AddressRangeExt, PhysicalAddress};

pub struct BootInfo<'dt> {
    pub boot_hart: u32,
    pub fdt: &'dt [u8],
    /// The number of "standalone" CPUs in the system
    pub cpus: usize,
    /// Address ranges we may use for allocation
    pub memories: ArrayVec<Range<PhysicalAddress>, 16>,
}

impl<'dt> fmt::Debug for BootInfo<'dt> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("BootInfo")
            .field("fdt", &self.fdt.as_ptr_range())
            .field("cpus", &self.cpus)
            .field("memories", &self.memories)
            .finish()
    }
}

impl<'dt> BootInfo<'dt> {
    pub fn from_dtb(dtb_ptr: *const u8) -> Self {
        let fdt = unsafe { DevTree::from_raw(dtb_ptr) }.unwrap();
        let mut reservations = fdt.reserved_entries();
        let fdt_slice = fdt.as_slice();
        let boot_hart = fdt.boot_cpuid_phys();

        let mut v = BootInfoVisitor::default();
        fdt.visit(&mut v).unwrap();

        let mut info = BootInfo {
            fdt: fdt_slice,
            boot_hart,
            cpus: v.cpus,
            memories: v.memories.regs,
        };

        let mut exclude_region = |entry: Range<PhysicalAddress>| {
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
        };

        // Apply memory reservations
        while let Some(entry) = reservations.next_entry().unwrap() {
            let entry = unsafe {
                let start = PhysicalAddress::new(entry.address as usize);

                start..start.add(entry.size as usize)
            };

            exclude_region(entry);
        }

        // Reserve the FDT region itself
        exclude_region(unsafe {
            let base = PhysicalAddress::new(info.fdt.as_ptr() as usize);

            (base..base.add(info.fdt.len())).align(kconfig::PAGE_SIZE)
        });

        // ensure the memory regions are sorted.
        // this is important for the allocation logic to be correct
        info.memories.sort_unstable_by(|a, b| {
            if a.end < b.start {
                Ordering::Less
            } else if b.end < a.start {
                Ordering::Greater
            } else {
                // This should never happen if the `exclude_region` code about is correct
                unreachable!("Memory region {a:?} and {b:?} are overlapping");
            }
        });

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

impl<'dt> Visitor<'dt> for BootInfoVisitor {
    type Error = dtb_parser::Error;
    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        if name == "cpus" || name.is_empty() {
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
