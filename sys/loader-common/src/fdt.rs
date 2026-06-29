// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::range::Range;

use arrayvec::ArrayVec;
use fallible_iterator::FallibleIterator;
use fdt::{Fdt, Node};
use loader_api::{MemoryRegion, MemoryRegionKind};
use mem_core::{AddressRangeExt, PhysicalAddress};

use crate::Error;
use crate::machine_info::DiscoveredUart;

pub fn boot_hartid(fdt: &Fdt) -> Option<usize> {
    usize::try_from(fdt.boot_cpuid()).ok()

    // // // TODO vs????
    // chosen.find_property("boot-hartid").ok()??.as_usize().ok()
}

pub fn rng_seed(chosen: &Node) -> Option<[u8; 32]> {
    let prop = chosen.find_property("rng-seed").ok()??;

    prop.raw.first_chunk::<32>().copied()
}

pub fn stdout_uart(fdt: &Fdt, chosen: &Node, page_size: usize) -> Option<DiscoveredUart> {
    let prop = chosen
        .find_property("stdout-path")
        .ok()
        .flatten()
        .or_else(|| chosen.find_property("linux,stdout-path").ok().flatten())?
        .as_str()
        .ok()?;

    // Split the node path from its optional `:options` tail; a leading `/` marks
    // a full path, anything else is an `/aliases` entry to dereference.
    let (head, mut options) = split_options(prop);

    let path = if head.starts_with('/') {
        head
    } else {
        let aliased = fdt
            .find_node("/aliases")
            .ok()??
            .find_property(head)
            .ok()??
            .as_str()
            .ok()?;
        let (path, alias_options) = split_options(aliased);
        options = options.or(alias_options);
        path
    };

    let node = fdt.find_node(path).ok()??;

    // `reg` → register block, decoded with the cell counts the `fdt` crate
    // resolved for this node. Its decoder panics on counts outside these ranges,
    // so reject them first.
    let cells = node.cell_sizes();
    if !matches!(cells.address_cells, 1 | 2) || cells.size_cells > 2 {
        return None;
    }
    let reg = node.reg().ok()??.next().ok()??;
    // Default to a single page when `reg` omits a size (`#size-cells = 0`).
    let regs = Range::from_start_len(
        PhysicalAddress::new(reg.starting_address),
        reg.size.unwrap_or(page_size),
    );

    // The baud divisor needs the input clock; without it we can't drive output.
    let clock_frequency = u32_prop(&node, "clock-frequency")?;

    // Baud rate: `stdout-path` options win, then `current-speed`, else 115200.
    let baud_rate = options
        .and_then(parse_baud)
        .or_else(|| u32_prop(&node, "current-speed"))
        .unwrap_or(115_200);

    // Register layout; the defaults describe a byte-addressed 16550.
    let reg_shift = u32_prop(&node, "reg-shift").unwrap_or(0);
    let reg_io_width = u32_prop(&node, "reg-io-width").unwrap_or(1);

    let irq_num = node.find_property("interrupts").ok()??.as_u32().ok()?;

    Some(DiscoveredUart {
        regs,
        clock_frequency,
        baud_rate,
        reg_shift,
        reg_io_width,
        irq_num,
    })
}

/// Split a `stdout-path` value into its node path and optional `:options` tail
/// (DeviceTree spec §3.6 — everything after the first `:` is firmware options).
fn split_options(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once(':') {
        Some((path, options)) => (path, Some(options)),
        None => (spec, None),
    }
}

/// Parse the leading decimal baud rate from an options string (`115200n8` → 115200).
fn parse_baud(options: &str) -> Option<u32> {
    options
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// Read a `u32`-typed property, or `None` if it is absent or the wrong shape.
fn u32_prop(node: &Node<'_>, name: &str) -> Option<u32> {
    node.find_property(name).ok()??.as_u32().ok()
}

/// Parse reported physical memory regions.
///
/// # Errors
///
/// Fails with `Err(Error::TooManyRegions)` if the firmware reported more than `N` physical memory regions.
pub fn physical_memory_regions<const N: usize>(
    fdt: &Fdt,
) -> crate::Result<ArrayVec<MemoryRegion, N>> {
    let mut memories = ArrayVec::<_, N>::new();
    let mut reserved = ArrayVec::<_, N>::new();

    // Depth of the `/reserved-memory` container while we're inside its subtree;
    // its direct children are the statically reserved regions.
    let mut reserved_root: Option<usize> = None;

    let mut nodes = fdt.nodes()?;
    while let Some((depth, node)) = nodes.next()? {
        // Left the reserved-memory subtree once we're back at or above its level.
        if reserved_root.is_some_and(|root| depth <= root) {
            reserved_root = None;
        }

        let name = node.name()?;
        if reserved_root == Some(depth - 1) {
            collect_regions(&node, &mut reserved)?;
        } else if name.name == "reserved-memory" {
            reserved_root = Some(depth);
        } else if name.name == "memory"
            && node
                .find_property("device_type")?
                .and_then(|p| p.as_cstr().ok())
                == Some(c"memory")
        {
            collect_regions(&node, &mut memories)?;
        }
    }

    let mut reservations = fdt.reserved_entries();
    while let Some(entry) = reservations.next()? {
        let start = PhysicalAddress::try_from(entry.address)
            .map_err(|_| fdt::Error::InvalidPropertyValue)?;
        let len = usize::try_from(entry.size).map_err(|_| fdt::Error::InvalidPropertyValue)?;

        let region = Range::from_start_len(start, len);

        log::trace!("applying reservation {region:#x?}");
        apply_reservation(&mut memories, region, MemoryRegionKind::Unusable)?;
    }
    for region in reserved {
        log::trace!("applying reservation {region:#x?}");
        apply_reservation(&mut memories, region.range, MemoryRegionKind::Unusable)?;
    }

    Ok(memories)
}

fn collect_regions<const N: usize>(
    node: &Node,
    out: &mut ArrayVec<MemoryRegion, N>,
) -> crate::Result<()> {
    let Some(mut regs) = node.reg()? else {
        return Ok(());
    };
    while let Some(reg) = regs.next()? {
        let range = Range::from_start_len(
            PhysicalAddress::new(reg.starting_address),
            reg.size.unwrap_or(0),
        );

        out.try_push(MemoryRegion {
            range,
            kind: MemoryRegionKind::Usable,
        })
        .map_err(|_| Error::TooManyRegions)?;
    }
    Ok(())
}

/// Apply a `reservation` with a given `kind` to the set of memory regions.
///
/// Will ensure the given reserved range _does not_ appear as usable AND is marked as unusable in the list.
///
/// # Errors
///
/// Fails with `Err(Error::TooManyRegions)`
pub fn apply_reservation<const N: usize>(
    regions: &mut ArrayVec<MemoryRegion, N>,
    reservation: Range<PhysicalAddress>,
    kind: MemoryRegionKind,
) -> crate::Result<()> {
    for i in (0..regions.len()).rev() {
        let r = regions[i].clone();

        if matches!(r.kind, MemoryRegionKind::Unusable) {
            continue;
        }

        if reservation.end <= r.range.start || r.range.end <= reservation.start {
            continue; // disjoint
        }

        regions.swap_remove(i);
        if r.range.start < reservation.start {
            let range = Range::from(r.range.start..reservation.start);

            regions
                .try_push(MemoryRegion {
                    range,
                    kind: MemoryRegionKind::Usable,
                })
                .map_err(|_| Error::TooManyRegions)?;
        }
        if reservation.end < r.range.end {
            let range = Range::from(reservation.end..r.range.end);

            regions
                .try_push(MemoryRegion {
                    range,
                    kind: MemoryRegionKind::Usable,
                })
                .map_err(|_| Error::TooManyRegions)?;
        }
    }

    regions
        .try_push(MemoryRegion {
            range: reservation,
            kind,
        })
        .map_err(|_| Error::TooManyRegions)?;

    Ok(())
}
