// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::cmp::Ordering;
use core::ffi::c_void;
use core::fmt::Formatter;
use core::range::Range;
use core::{fmt, mem};

use arrayvec::ArrayVec;
use fallible_iterator::FallibleIterator;
use fdt::{CellSizes, Fdt, PropertiesIter};
use mem_core::{AddressRangeExt, PhysicalAddress};

use crate::arch::PAGE_SIZE;
use crate::error::Error;

/// Information about the machine we're running on.
/// This is collected from the FDT (flatting device tree) passed to us by the previous stage loader.
pub struct MachineInfo<'dt> {
    /// The FDT blob passed to us by the previous stage loader
    pub fdt: &'dt [u8],
    /// Address ranges we may use for allocation
    pub memories: ArrayVec<Range<PhysicalAddress>, 16>,
    /// The RNG seed passed to us by the previous stage loader.
    pub rng_seed: Option<&'dt [u8]>,
}

impl MachineInfo<'_> {
    pub unsafe fn from_dtb(fdt_ptr: *const c_void) -> crate::Result<Self> {
        assert!(!fdt_ptr.is_null());
        assert_eq!(fdt_ptr.align_offset(align_of::<u32>()), 0); // make sure the pointer is aligned correctly

        // Safety: we made a reasonable effort to ensure the pointer is valid
        let fdt = unsafe { Fdt::from_ptr(fdt_ptr.cast())? };
        let mut reservations = fdt.reserved_entries();
        let fdt_slice = fdt.as_slice();

        let mut memories: ArrayVec<Range<PhysicalAddress>, 16> = ArrayVec::new();
        let mut reserved_memory: ArrayVec<Range<PhysicalAddress>, 16> = ArrayVec::new();
        let mut rng_seed = None;

        let mut stack: [Option<(&str, CellSizes)>; 16] = [const { None }; 16];
        stack[0] = Some((
            "",
            find_size_cells(fdt.properties(), &CellSizes::default())?,
        ));

        let mut iter = fdt.nodes()?;
        while let Some((depth, node)) = iter.next()? {
            let name = node.name()?;

            if name.name == "memory"
                && node
                    .find_property("device_type")?
                    .and_then(|p| p.as_cstr().ok())
                    == Some(c"memory")
            {
                // if the node is a memory node, add it to the list of available memory regions

                let mut iter = node
                    .find_property("reg")?
                    .unwrap()
                    .as_regs(stack[depth - 1].unwrap().1);

                while let Some(reg) = iter.next()? {
                    memories.push({
                        let start = PhysicalAddress::new(reg.starting_address);

                        Range::from_start_len(start, reg.size.unwrap_or(0))
                    });
                }
            } else if stack[depth - 1].is_some_and(|(s, _)| s == "reserved-memory") {
                // if the node is a reserved-memory node, add it to the list of reserved memory regions

                let mut iter = node
                    .find_property("reg")?
                    .unwrap()
                    .as_regs(stack[depth - 1].unwrap().1);
                while let Some(reg) = iter.next()? {
                    reserved_memory.push({
                        let start = PhysicalAddress::new(reg.starting_address);

                        Range::from_start_len(start, reg.size.unwrap_or(0))
                    });
                }
            } else if name.name == "chosen" {
                // and finally if the node is the chosen node, extract the RNG seed

                rng_seed = node.find_property("rng-seed")?.map(|prop| prop.raw);
            }

            // add the name and size_cells to the stack so we have it available for the next iteration
            stack[depth] = Some((
                name.name,
                find_size_cells(node.properties(), &stack[depth - 1].as_ref().unwrap().1)?,
            ));
        }

        let mut exclude_region = |entry: Range<PhysicalAddress>| {
            let _memories = mem::take(&mut memories);

            for mut region in _memories {
                if entry.contains(&region.start) && entry.contains(&region.end) {
                    // remove region
                    continue;
                } else if region.contains(&entry.start) && region.contains(&entry.end) {
                    memories.push(Range::from(region.start..entry.start));
                    memories.push(Range::from(entry.end..region.end));
                } else if entry.contains(&region.start) {
                    region.start = entry.end;
                    memories.push(region);
                } else if entry.contains(&region.end) {
                    region.end = entry.start;
                    memories.push(region);
                } else {
                    memories.push(region);
                }
            }
        };

        // Apply reserved_entries
        while let Some(entry) = reservations.next()? {
            let region = Range::from_start_len(
                PhysicalAddress::try_from(entry.address)?,
                usize::try_from(entry.size)?,
            );

            log::trace!("applying reservation {region:#x?}");

            exclude_region(region);
        }

        // Apply memory reservations
        for reservation in reserved_memory {
            log::trace!("applying reservation {reservation:#x?}");

            exclude_region(reservation);
        }

        // remove memory regions that are left as zero-sized from the previous step
        memories.retain(|region| !region.is_empty());

        // page-align all memory regions, this will waste some physical memory in the process,
        // but we can't make use of it either way
        memories.iter_mut().for_each(|region| {
            *region = region.align_in(PAGE_SIZE);
        });

        // ensure the memory regions are sorted.
        // this is important for the allocation logic to be correct
        memories.sort_unstable_by(|a, b| -> Ordering {
            if a.end <= b.start {
                Ordering::Less
            } else if b.end <= a.start {
                Ordering::Greater
            } else {
                // This should never happen if the `exclude_region` code about is correct
                unreachable!("Memory region {a:?} and {b:?} are overlapping");
            }
        });

        Ok(MachineInfo {
            fdt: fdt_slice,
            memories,
            rng_seed,
        })
    }

    /// Returns the *convex hull* of all physical memory regions i.e. the smallest range of addresses
    /// that contains all memory regions.
    ///
    /// Since we *could* have multiple memory regions, and those regions need not be contiguous,
    /// this function should be used to determine the range of addresses that we should map in the
    /// [`map_physical_memory`][crate::mapping::map_physical_memory] step.
    pub fn memory_hull(&self) -> Range<PhysicalAddress> {
        // This relies on the memory regions being sorted by the constructor
        debug_assert!(self.memories.is_sorted_by(|a, b| { a.end <= b.start }));

        let min_addr = self.memories.first().map(|r| r.start).unwrap_or_default();
        let max_addr = self.memories.last().map(|r| r.end).unwrap_or_default();

        Range::from(min_addr..max_addr)
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

fn find_size_cells(mut iter: PropertiesIter, parent: &CellSizes) -> Result<CellSizes, Error> {
    let mut address_cells = parent.address_cells;
    let mut size_cells = parent.size_cells;

    while let Some(prop) = iter.next()? {
        match prop.name {
            "#address-cells" => {
                address_cells = prop.as_usize()?;
            }
            "#size-cells" => {
                size_cells = prop.as_usize()?;
            }
            _ => {}
        }
    }

    Ok(CellSizes {
        address_cells,
        size_cells,
    })
}
