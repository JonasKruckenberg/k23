use crate::arch;
use arrayvec::ArrayVec;
use core::cmp::Ordering;
use core::ffi::CStr;
use core::fmt;
use core::fmt::Formatter;
use core::ops::Range;
use dtb_parser::{DevTree, Node, Visitor};
use mmu::{AddressRangeExt, PhysicalAddress};

/// Information about the machine we're running on.
/// This is collected from the FDT (flatting device tree) passed to us by the previous stage loader.
pub struct MachineInfo<'dt> {
    /// The FDT blob passed to us by the previous stage loader
    pub fdt: &'dt [u8],
    /// The number of "standalone" CPUs in the system
    pub cpus: usize,
    /// A bitfield where each bit corresponds to a CPU in the system.
    /// A `1` bit indicates the CPU is "online" and can be used,
    ///     while a `0` bit indicates the CPU is "offline" and can't be used by the system.
    /// This is used across SBI calls to dispatch IPIs to the correct CPUs.
    pub hart_mask: usize,
    /// Address ranges we may use for allocation
    pub memories: ArrayVec<Range<PhysicalAddress>, 16>,
    /// The boot arguments passed to us by the previous stage loader.
    pub bootargs: Option<&'dt CStr>,
    /// The RNG seed passed to us by the previous stage loader.
    pub rng_seed: Option<&'dt [u8]>,
}

impl<'dt> MachineInfo<'dt> {
    /// Returns the *convex hull* of all physical memory regions i.e. the smallest range of addresses
    /// that contains all memory regions.
    ///
    /// Since we *could* have multiple memory regions, and those regions need not be contiguous,
    /// this function should be used to determine the range of addresses that we should map in the
    /// [`map_physical_memory`][crate::PageTableBuilder::map_physical_memory] step.
    pub fn memory_hull(&self) -> Range<PhysicalAddress> {
        // This relies on the memory regions being sorted by the constructor
        debug_assert!(self.memories.is_sorted_by(|a, b| { a.end <= b.start }));

        let min_addr = self.memories.first().map(|r| r.start).unwrap_or_default();
        let max_addr = self.memories.last().map(|r| r.end).unwrap_or_default();

        min_addr..max_addr
    }
}

impl<'dt> fmt::Debug for MachineInfo<'dt> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("MachineInfo")
            .field("fdt", &self.fdt.as_ptr_range())
            .field("cpus", &self.cpus)
            .field("hart_mask", &format_args!("{:b}", self.hart_mask))
            .field("memories", &self.memories)
            .field("bootargs", &self.bootargs)
            .field("rng_seed", &self.rng_seed)
            .finish()
    }
}

impl fmt::Display for MachineInfo<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{:<17} : {:?}",
            "DEVICE TREE BLOB",
            self.fdt.as_ptr_range()
        )?;
        writeln!(f, "{:<17} : {}", "CPUS", self.cpus)?;
        writeln!(f, "{:<17} : {}", "HART MASK", self.hart_mask)?;
        if let Some(bootargs) = self.bootargs {
            writeln!(f, "{:<17} : {:?}", "BOOTARGS", bootargs)?;
        } else {
            writeln!(f, "{:<17} : None", "BOOTARGS")?;
        }
        if let Some(rng_seed) = self.rng_seed {
            writeln!(f, "{:<17} : {:?}", "PRNG SEED", rng_seed)?;
        } else {
            writeln!(f, "{:<17} : None", "PRNG SEED")?;
        }

        for (idx, r) in self.memories.iter().enumerate() {
            writeln!(f, "MEMORY REGION {:<4}: {}..{}", idx, r.start, r.end)?;
        }

        Ok(())
    }
}

#[derive(Debug)]
enum MemoryReservation<'dt> {
    NoMap(&'dt str, ArrayVec<Range<PhysicalAddress>, 16>),
}

impl<'dt> MachineInfo<'dt> {
    pub unsafe fn from_dtb(dtb_ptr: *const u8) -> crate::Result<Self> {
        let fdt = unsafe { DevTree::from_raw(dtb_ptr) }?;
        let mut reservations = fdt.reserved_entries();
        let fdt_slice = fdt.as_slice();

        let mut v = MachineInfoVisitor::default();
        fdt.visit(&mut v).unwrap();

        let mut info = MachineInfo {
            fdt: fdt_slice,
            cpus: v.cpus.cpus,
            hart_mask: v.cpus.hart_mask,
            memories: v.memories.regs,
            bootargs: v.chosen_visitor.bootargs,
            rng_seed: v.chosen_visitor.rng_seed,
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
                } else {
                    info.memories.push(region);
                }
            }
        };

        // Apply reserved_entries
        while let Some(entry) = reservations.next_entry()? {
            let entry = {
                let start = PhysicalAddress::new(usize::try_from(entry.address)?);

                start..start.checked_add(usize::try_from(entry.size)?).unwrap()
            };

            exclude_region(entry);
        }

        // Apply memory reservations
        for reservation in v.reservations.memory_reservations {
            let MemoryReservation::NoMap(name, regions) = reservation;
            log::trace!("applying reservations for {name}");

            for region in regions {
                exclude_region(region);
            }
        }

        // remove memory regions that are left as zero-sized from the previous step
        info.memories.retain(|region| region.size() > 0);

        // page-align all memory regions, this will waste some physical memory in the process,
        // but we can't make use of it either way
        info.memories.iter_mut().for_each(|region| {
            region.start = region.start.checked_align_up(arch::PAGE_SIZE).unwrap();
            region.end = region.end.align_down(arch::PAGE_SIZE);
        });

        // ensure the memory regions are sorted.
        // this is important for the allocation logic to be correct
        info.memories.sort_unstable_by(compare_memory_regions);

        Ok(info)
    }
}

fn compare_memory_regions(a: &Range<PhysicalAddress>, b: &Range<PhysicalAddress>) -> Ordering {
    if a.end <= b.start {
        Ordering::Less
    } else if b.end <= a.start {
        Ordering::Greater
    } else {
        // This should never happen if the `exclude_region` code about is correct
        unreachable!("Memory region {a:?} and {b:?} are overlapping");
    }
}

/*--------------------------------------------------------------------------------------------------
    visitors
---------------------------------------------------------------------------------------------------*/
#[derive(Default)]
struct MachineInfoVisitor<'dt> {
    cpus: CpusVisitor,
    memories: RegsVisitor,
    reservations: ReservationsVisitor<'dt>,
    chosen_visitor: ChosenVisitor<'dt>,
}

impl<'dt> Visitor<'dt> for MachineInfoVisitor<'dt> {
    type Error = dtb_parser::Error;
    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        if name.is_empty() {
            node.visit(self)?;
        } else if name == "cpus" {
            node.visit(&mut self.cpus)?;
        } else if name.starts_with("memory@") {
            node.visit(&mut self.memories)?;
        } else if name == "reserved-memory" {
            node.visit(&mut self.reservations)?;
        } else if name == "chosen" {
            node.visit(&mut self.chosen_visitor)?;
        }

        Ok(())
    }

    fn visit_address_cells(&mut self, size_in_cells: u32) -> Result<(), Self::Error> {
        let size_in_bytes = size_in_cells as usize * size_of::<u32>();

        self.memories.address_size = size_in_bytes;
        self.reservations.address_size = size_in_bytes;

        Ok(())
    }

    fn visit_size_cells(&mut self, size_in_cells: u32) -> Result<(), Self::Error> {
        let size_in_bytes = size_in_cells as usize * size_of::<u32>();

        self.memories.width_size = size_in_bytes;
        self.reservations.width_size = size_in_bytes;

        Ok(())
    }
}

#[derive(Default)]
struct RegsVisitor {
    address_size: usize,
    width_size: usize,
    regs: ArrayVec<Range<PhysicalAddress>, 16>,
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

            self.regs.push(start..start.checked_add(width).unwrap());
        }

        Ok(())
    }
}

#[derive(Default)]
struct ReservationsVisitor<'dt> {
    address_size: usize,
    width_size: usize,
    memory_reservations: ArrayVec<MemoryReservation<'dt>, 16>,
}

impl<'dt> ReservationsVisitor<'dt> {
    fn subnode_visitor(&self, name: &'dt str) -> ReservationVisitor<'dt> {
        ReservationVisitor {
            name,
            no_map: false,
            regs_visitor: RegsVisitor {
                address_size: self.address_size,
                width_size: self.width_size,
                regs: ArrayVec::default(),
            },
        }
    }
}

impl<'dt> Visitor<'dt> for ReservationsVisitor<'dt> {
    type Error = dtb_parser::Error;

    fn visit_subnode(&mut self, name: &'dt str, node: Node<'dt>) -> Result<(), Self::Error> {
        let mut v = self.subnode_visitor(name);
        node.visit(&mut v)?;
        self.memory_reservations.push(v.result());

        Ok(())
    }
}

struct ReservationVisitor<'dt> {
    no_map: bool,
    regs_visitor: RegsVisitor,
    name: &'dt str,
}

impl<'dt> ReservationVisitor<'dt> {
    fn result(self) -> MemoryReservation<'dt> {
        assert!(self.no_map);

        MemoryReservation::NoMap(self.name, self.regs_visitor.regs)
    }
}

impl<'dt> Visitor<'dt> for ReservationVisitor<'dt> {
    type Error = dtb_parser::Error;

    fn visit_reg(&mut self, reg: &'dt [u8]) -> Result<(), Self::Error> {
        self.regs_visitor.visit_reg(reg)
    }

    fn visit_property(&mut self, name: &'dt str, _value: &'dt [u8]) -> Result<(), Self::Error> {
        match name {
            // Indicates the operating system must not create a virtual mapping of the region as part
            // of its standard mapping of system memory, nor permit speculative access to it under
            // any circumstances other than under the control of the device driver using the region.
            "no-map" => self.no_map = true,
            // Size in bytes of memory to reserve for dynamically allocated regions.
            // Size of this property is based on parent node's #size-cells property.
            "size" => todo!(),
            // Address boundary for alignment of allocation. Size of this property is based on parent
            // node's #size-cells property.
            "alignment" => todo!(),
            // Specifies regions of memory that are acceptable to allocate from.
            // Format is (address, length pairs) tuples in same format as for reg properties.
            "alloc-ranges" => todo!(),
            // The operating system can use the memory in this region with the limitation that the
            // device driver(s) owning the region need to be able to reclaim it back.
            // Typically, that means that the operating system can use that region to store volatile
            // or cached data that can be otherwise regenerated or migrated elsewhere.
            "reusable" => todo!(),
            _ => unimplemented!(),
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

impl<'dt> CpuVisitor<'dt> {
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
