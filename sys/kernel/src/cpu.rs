// Copyright 2023-Present Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Which CPU is this, and which CPUs are there.
//!
//! [`LogicalCpuId`] and `MAX_CPUS` come from `//sys/cpus`. The two things that
//! crate cannot answer live here: the calling CPU's id, which is a CPU-local
//! read, and the mapping to the hardware ids the device tree and SBI speak in.

use core::cell::Cell;

use anyhow::Context;
use arrayvec::ArrayVec;
use cpu_local::cpu_local;
pub use cpus::{LogicalCpuId, MAX_CPUS};

use crate::device_tree::{Device, DeviceTree};

/// The CPU the loader handed control to.
///
/// The boot CPU takes id zero by definition rather than by enumeration, so that
/// a [`LogicalCpuId`] exists from the first instruction of `kmain` — long before there
/// is a device tree to enumerate.
pub const BOOT: LogicalCpuId = LogicalCpuId::new(0);

/// Held by [`CURRENT`] until this CPU is assigned an id. Ids are dense and
/// bounded by `MAX_CPUS`, so the top of the range is free as a sentinel.
const UNASSIGNED: u32 = u32::MAX;

cpu_local! {
    /// The calling CPU's id, raw so the whole key stays one word.
    static CURRENT: Cell<u32> = const { Cell::new(UNASSIGNED) };
}

/// The calling CPU's id, or `None` if it has not been assigned one yet.
///
/// Panic-free: tracing tags every line with this, including lines written
/// before per-CPU state exists and from the panic handler.
pub fn try_current() -> Option<LogicalCpuId> {
    match CURRENT.get() {
        UNASSIGNED => None,
        raw => Some(LogicalCpuId::new(raw)),
    }
}

/// The calling CPU's id.
///
/// # Panics
///
/// Panics if the calling CPU has not been assigned an id. Use [`try_current`]
/// on paths that can run before [`make_current`].
pub fn current() -> LogicalCpuId {
    try_current().expect("CPU id read before it was assigned; missing `cpu::make_current`")
}

/// Make `id` the value [`current`] returns on the calling CPU.
///
/// Called once per CPU, before anything that indexes a per-CPU table runs.
pub fn make_current(id: LogicalCpuId) {
    debug_assert_eq!(CURRENT.get(), UNASSIGNED, "CPU id assigned twice");
    CURRENT.set(id.as_u32());
}

/// The CPUs the kernel knows about, and the mapping between their dense
/// [`LogicalCpuId`]s and their hardware ids.
///
/// Built once during boot and never mutated, so a CPU that goes offline keeps
/// its id — and therefore its per-CPU state — for when it comes back.
#[derive(Debug)]
pub struct Cpus {
    /// Hardware ids indexed by [`LogicalCpuId`]; entry zero is the boot CPU.
    hartids: ArrayVec<usize, MAX_CPUS>,
}

impl Cpus {
    /// Enumerate `/cpus`, assigning every hart a dense [`LogicalCpuId`].
    ///
    /// The boot hart is [`BOOT`]; the rest follow in device-tree order.
    ///
    /// # Errors
    ///
    /// Returns an error if the device tree has no `/cpus` node, or if a `cpu`
    /// node's hardware id can't be read.
    pub fn from_device_tree(devtree: &DeviceTree, boot_hartid: usize) -> crate::Result<Self> {
        let cpus = devtree
            .find_by_path("/cpus")
            .context("device tree has no /cpus node")?;

        let mut this = Self {
            hartids: ArrayVec::new(),
        };
        this.add(boot_hartid);

        // Harts past `MAX_CPUS` are dropped rather than fatal: booting on the
        // subset of a machine the kernel was configured for beats refusing to
        // boot on it at all.
        let mut dropped = 0usize;
        for node in cpus.children(devtree).filter(|dev| dev.name.name == "cpu") {
            if !this.add(hartid_of(devtree, node)?) {
                dropped += 1;
            }
        }

        if dropped > 0 {
            tracing::warn!("{dropped} harts past MAX_CPUS ({MAX_CPUS}) stay offline");
        }

        Ok(this)
    }

    /// The number of CPUs in the system, one past the largest [`LogicalCpuId`] in use.
    pub fn len(&self) -> usize {
        self.hartids.len()
    }

    /// The hardware id of `id`.
    ///
    /// # Panics
    ///
    /// Panics if `id` was not assigned by this `Cpus`.
    pub fn hartid(&self, id: LogicalCpuId) -> usize {
        self.hartids[id.as_usize()]
    }

    /// The dense id of hart `hartid`, or `None` if the kernel isn't tracking
    /// that hart.
    pub fn cpu_id(&self, hartid: usize) -> Option<LogicalCpuId> {
        // A scan of at most `MAX_CPUS` words, walked only when a CPU crosses
        // the hardware boundary. A reverse table would have to be indexed by a
        // sparse, unbounded hart id — the very thing `LogicalCpuId` exists to avoid.
        self.iter().find(|&(_, h)| h == hartid).map(|(id, _)| id)
    }

    /// Every CPU, as a `(dense id, hardware id)` pair.
    pub fn iter(&self) -> impl Iterator<Item = (LogicalCpuId, usize)> + '_ {
        (0u32..)
            .map(LogicalCpuId::new)
            .zip(self.hartids.iter().copied())
    }

    /// Give `hartid` the next free [`LogicalCpuId`], reporting whether it has one.
    ///
    /// A hart that already has an id keeps it — the boot hart also turns up in
    /// the device-tree walk — so only running out of slots returns `false`.
    fn add(&mut self, hartid: usize) -> bool {
        self.cpu_id(hartid).is_some() || self.hartids.try_push(hartid).is_ok()
    }
}

/// The hardware id of a `/cpus/cpu@…` node.
///
/// Read from `reg`, not from the unit address: the unit address is only a
/// rendering of `reg` (DeviceTree spec v0.4 §2.2.1) and is written in hex, so
/// parsing it works right up until a machine has more than ten harts.
/// `/cpus` declares `#address-cells = <1>` and `#size-cells = <0>`, so `reg` is
/// the hart id on its own.
///
/// # Errors
///
/// Returns an error if the node has no `reg` property, or it isn't a cell value.
pub fn hartid_of(devtree: &DeviceTree, node: Device<'_>) -> crate::Result<usize> {
    let reg = node
        .property(devtree, "reg")
        .context("cpu node has no reg property")?;

    Ok(reg.as_usize()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Cpus` without a device tree, so the assignment can be driven with
    /// hart ids no emulator would hand us.
    fn cpus(boot_hartid: usize, rest: impl IntoIterator<Item = usize>) -> Cpus {
        let mut cpus = Cpus {
            hartids: ArrayVec::new(),
        };
        cpus.add(boot_hartid);
        for hartid in rest {
            cpus.add(hartid);
        }
        cpus
    }

    #[test::test]
    async fn sparse_hartids_become_dense_ids() {
        let cpus = cpus(0, [0, 7, 1000, 42]);

        assert_eq!(cpus.len(), 4);
        assert!(cpus.iter().eq([
            (LogicalCpuId::new(0), 0),
            (LogicalCpuId::new(1), 7),
            (LogicalCpuId::new(2), 1000),
            (LogicalCpuId::new(3), 42)
        ]));
    }

    #[test::test]
    async fn boot_hart_is_cpu_zero_whatever_its_hartid() {
        // The boot hart need not come first in the device tree.
        let cpus = cpus(1000, [7, 1000, 42]);

        assert_eq!(cpus.cpu_id(1000), Some(BOOT));
        assert_eq!(cpus.hartid(BOOT), 1000);
        // ...and it is not handed a second slot on the way past.
        assert_eq!(cpus.len(), 3);
    }

    #[test::test]
    async fn ids_and_hartids_round_trip() {
        let cpus = cpus(3, [1, 3, 5]);

        for (id, hartid) in cpus.iter() {
            assert_eq!(cpus.hartid(id), hartid);
            assert_eq!(cpus.cpu_id(hartid), Some(id));
        }

        assert_eq!(cpus.cpu_id(4), None);
    }

    #[test::test]
    async fn harts_past_max_cpus_are_dropped_not_fatal() {
        // Sparse hart ids, so the ones that overflow are also the far-away ones.
        let cpus = cpus(0, (1..).map(|h| h * 3).take(MAX_CPUS * 2));

        assert_eq!(cpus.len(), MAX_CPUS);
        assert_eq!(cpus.hartid(BOOT), 0);
        assert_eq!(cpus.cpu_id(MAX_CPUS * 3), None);
    }
}
