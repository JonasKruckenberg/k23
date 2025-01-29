// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch;
use crate::hart_local::HartLocal;
use crate::vm::PhysicalAddress;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::ffi::CStr;
use core::num::NonZero;
use core::ops::ControlFlow;
use core::range::Range;
use core::str::FromStr;
use fallible_iterator::FallibleIterator;
use fdt::{Fdt, Node, NodePath};
use hashbrown::HashMap;
use sync::OnceLock;

static MACHINE_INFO: OnceLock<MachineInfo<'static>> = OnceLock::new();

#[derive(Debug)]
pub struct MachineInfo<'dt> {
    pub fdt: Fdt<'dt>,
    /// The boot arguments passed to us by the previous stage loader.
    pub bootargs: Option<&'dt CStr>,
    /// The RNG seed passed to us by the previous stage loader.
    pub rng_seed: Option<&'dt [u8]>,
    pub hart_local: HartLocal<HartLocalMachineInfo>,
    pub interrupt_controllers: HashMap<u32, InterruptController<'dt>>,
    pub atlas: NodeAtlas<'dt>,
}

#[derive(Debug)]
pub struct HartLocalMachineInfo {
    /// Timebase frequency of the hart in Hertz.
    pub timebase_frequency: u64,
    pub numa_node_id: usize,
    pub arch: arch::machine_info::HartLocalMachineInfo,
}

/// An interrupt source that can be wired to a parent interrupt controller.
/// The number of cells in the interrupt specifier is determined by the parent interrupt controller.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IrqSource {
    /// RISC-V seems to exclusively use 1-cell interrupt sources.
    C1(u32),
    /// AArch64 uses exclusively 3-cell interrupt sources.
    C3(u32, u32, u32),
}

// TODO better name
#[derive(Debug)]
pub struct InterruptController<'dt> {
    pub compatible: &'dt str,
    pub numa_node_id: usize,
    pub mmio_regions: Vec<Range<PhysicalAddress>>,
    pub children: HashMap<IrqSource, u32>,
    pub parents: Vec<(u32, IrqSource)>,
}

pub fn machine_info() -> &'static MachineInfo<'static> {
    MACHINE_INFO.get().expect("MachineInfo not initialized")
}

pub fn init(fdt: &'static [u8]) -> crate::Result<&'static MachineInfo<'static>> {
    MACHINE_INFO.get_or_try_init(|| {
        // Safety: u32 has no invalid bit patterns
        let (left, aligned, _) = unsafe { fdt.align_to::<u32>() };
        assert!(left.is_empty()); // TODO decide what to do with unaligned slices
        let fdt = Fdt::new(aligned)?;

        // collect all
        let atlas = NodeAtlas::from_fdt(&fdt, |path, _| {
            path.starts_with("/chosen") || path.starts_with("/cpus") || path.starts_with("/soc")
        })?;

        let mut bootargs = None;
        let mut rng_seed = None;
        if let Some(node) = atlas.find_node("/chosen") {
            bootargs = node
                .property("bootargs")
                .and_then(|prop| prop.as_cstr().ok());
            rng_seed = node.property("rng-seed").map(|prop| prop.raw);
        }

        let mut hart_local: HartLocal<HartLocalMachineInfo> = HartLocal::new();
        let mut default_timebase_frequency: Option<u64> = None;
        for (id, node) in atlas.find_nodes("/cpus").unwrap() {
            let name = node.name()?;
            if name.name == "cpus" {
                default_timebase_frequency = node
                    .property("timebase-frequency")
                    .and_then(|prop| prop.as_u64().ok());
            }

            if name.name == "cpu"
                && let Some(hartid) = name.unit_address.and_then(|s| usize::from_str(s).ok())
            {
                let timebase_frequency = node
                    .property("timebase-frequency")
                    .and_then(|prop| prop.as_u64().ok())
                    .or(default_timebase_frequency)
                    .expect("RISC-V system with no 'timebase-frequency' in FDT");
                let numa_node_id = node
                    .property("numa-node-id")
                    .and_then(|prop| prop.as_usize().ok())
                    .unwrap_or_default();

                hart_local.insert_for(
                    hartid,
                    HartLocalMachineInfo {
                        timebase_frequency,
                        numa_node_id,
                        arch: arch::machine_info::parse_hart_local(&atlas, id, node),
                    },
                );
            }
        }

        let mut interrupt_controllers: HashMap<u32, InterruptController> = HashMap::new();
        for (_, node) in atlas.iter() {
            if node.property("interrupt-controller").is_some() {
                let phandle = node.property("phandle").unwrap().as_u32().unwrap();

                let mut mmio_regions = Vec::new();
                if let Some(regs) = node.reg() {
                    for reg in regs.unwrap() {
                        let start = PhysicalAddress::new(reg.starting_address);
                        let end = start.checked_add(reg.size.unwrap()).unwrap();
                        mmio_regions.push(Range::from(start..end));
                    }
                }

                let mut ctl = InterruptController {
                    compatible: node.property("compatible").unwrap().as_str().unwrap(),
                    numa_node_id: node
                        .property("numa-node-id")
                        .map(|prop| prop.as_usize().unwrap())
                        .unwrap_or_default(),
                    mmio_regions,
                    children: HashMap::new(),
                    parents: Vec::new(),
                };

                if let Some(intr_data) = node.property("interrupts-extended") {
                    let mut intr_data = intr_data
                        .raw
                        .array_chunks::<4>()
                        .map(|x| u32::from_be_bytes(*x));

                    log::debug!("interrupts-extended begin:");
                    while let Some(parent_phandle) = intr_data.next()
                        && let Some(parent) = atlas.find_by_phandle(parent_phandle)
                        && let Some(parent_interrupt_cells) = parent.interrupt_cells()
                        && let Some(interrupt) =
                            interrupt_source(&mut intr_data, parent_interrupt_cells)
                    {
                        log::debug!("interrupt {interrupt:?} is wired to node {parent:?}",);

                        interrupt_controllers
                            .get_mut(&parent_phandle)
                            .unwrap()
                            .children
                            .insert(interrupt.clone(), phandle);
                        ctl.parents.push((parent_phandle, interrupt));
                    }
                    log::debug!("interrupts-extended end");
                } else if let (Some(interrupt_parent), Some(intr_data)) = (
                    node.property("interrupt-parent"),
                    node.property("interrupts"),
                ) {
                    let parent_phandle = interrupt_parent.as_u32().unwrap();
                    let parent = atlas.find_by_phandle(parent_phandle).unwrap();
                    let parent_interrupt_cells = parent.interrupt_cells().unwrap();

                    let mut intr_data = intr_data
                        .raw
                        .array_chunks::<4>()
                        .map(|x| u32::from_be_bytes(*x));

                    log::debug!("interrupts begin:");
                    while let Some(interrupt) =
                        interrupt_source(&mut intr_data, parent_interrupt_cells)
                    {
                        log::debug!("interrupt {interrupt:?} is wired to node {parent:?}",);

                        interrupt_controllers
                            .get_mut(&parent_phandle)
                            .unwrap()
                            .children
                            .insert(interrupt.clone(), phandle);
                        ctl.parents.push((parent_phandle, interrupt));
                    }
                    log::debug!("interrupts end");
                }

                interrupt_controllers.insert(phandle, ctl);
            }
        }

        #[expect(tail_expr_drop_order, reason = "")]
        Ok(MachineInfo {
            fdt,
            bootargs,
            rng_seed,
            hart_local,
            interrupt_controllers,
            atlas,
        })
    })
}

fn interrupt_source(
    iter: &mut impl Iterator<Item = u32>,
    interrupt_cells: usize,
) -> Option<IrqSource> {
    match interrupt_cells {
        1 => Some(IrqSource::C1(iter.next()?)),
        3 if let Ok([a, b, c]) = iter.next_chunk() => Some(IrqSource::C3(a, b, c)),
        _ => None,
    }
}

fn is_supervisor_irq_source(source: &IrqSource) -> bool {
    match source {
        IrqSource::C1(u32::MAX) | IrqSource::C3(u32::MAX, _, _) => false,
        // FIXME(RISCV): OpenSBI apparently doesn't support multiple PLICs and so only
        //  rewrites the first PLICs machine-level interrupt sources to -1.
        IrqSource::C1(0xb) => false,
        _ => true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeId([Option<NonZero<usize>>; 6]);

impl NodeId {
    fn depth(&self) -> usize {
        self.0.iter().filter(|s| s.is_some()).count() - 1
    }

    pub fn append<'dt>(mut self, atlas: &NodeAtlas<'dt>, path: &'dt str) -> Option<NodeId> {
        let start = self.depth() + 1;
        for (idx, str) in path.split('/').skip(1).enumerate() {
            self.0[start + idx] = Some(*atlas.string2idx.get(str)?);
        }

        Some(self)
    }
}

#[derive(Default, Debug)]
pub struct NodeAtlas<'dt> {
    nodes: BTreeMap<NodeId, Node<'dt>>,
    phandle_to_nodeid: HashMap<u32, NodeId>,
    string2idx: HashMap<&'dt str, NonZero<usize>>,
    strings: Vec<&'dt str>,
}

impl<'dt> NodeAtlas<'dt> {
    pub fn from_fdt<F>(fdt: &Fdt<'dt>, mut filter: F) -> crate::Result<Self>
    where
        F: FnMut(&NodePath<'_, 'dt>, &Node<'dt>) -> bool,
    {
        let mut atlas = Self::default();

        fdt.walk(|path, node| -> ControlFlow<()> {
            if filter(&path, &node) {
                atlas.insert(path, node);
            }
            ControlFlow::Continue(())
        })?;

        Ok(atlas)
    }

    pub fn insert(&mut self, path: NodePath<'_, 'dt>, node: Node<'dt>) -> NodeId {
        let mut id = NodeId([None; 6]);

        for (idx, str) in path.into_iter().skip(1).enumerate() {
            id.0[idx] = Some(self.intern_str(str));

            self.intern_str(str);
        }

        if let Some(phandle) = node.property("phandle") {
            self.phandle_to_nodeid.insert(phandle.as_u32().unwrap(), id);
        }
        self.nodes.insert(id, node);

        id
    }

    pub(crate) fn get(&self, id: &NodeId) -> Option<&Node<'dt>> {
        self.nodes.get(id)
    }

    pub fn find_node(&self, path: &'dt str) -> Option<&Node<'dt>> {
        let mut id = NodeId([None; 6]);

        for (idx, str) in path.split('/').skip(1).enumerate() {
            id.0[idx] = Some(*self.string2idx.get(str)?);
        }

        self.nodes.get(&id)
    }

    pub fn find_nodes(
        &self,
        path: &'dt str,
    ) -> Option<impl Iterator<Item = (&NodeId, &Node<'dt>)>> {
        let mut start = NodeId([None; 6]);
        let mut depth = 0;

        for (idx, str) in path.split('/').skip(1).enumerate() {
            start.0[idx] = Some(*self.string2idx.get(str)?);
            depth = idx;
        }

        let mut end = start;
        end.0[depth + 1] = Some(NonZero::<usize>::MAX);

        Some(self.nodes.range(start..end))
    }

    pub fn find_children(
        &self,
        path: &'dt str,
    ) -> Option<impl Iterator<Item = (&NodeId, &Node<'dt>)>> {
        let mut start = NodeId([None; 6]);
        let mut depth = 0;

        for (idx, str) in path.split('/').skip(1).enumerate() {
            start.0[idx] = Some(*self.string2idx.get(str)?);
            depth = idx;
        }

        let mut end = start;
        end.0[depth + 1] = Some(NonZero::<usize>::MAX);

        Some(
            self.nodes
                .range(start..end)
                .filter(move |(id, _)| id.depth() == depth + 1),
        )
    }

    pub fn find_by_phandle(&self, phandle: u32) -> Option<&Node<'dt>> {
        self.phandle_to_nodeid
            .get(&phandle)
            .and_then(|id| self.nodes.get(id))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&NodeId, &Node<'dt>)> {
        self.nodes.iter()
    }

    fn intern_str(&mut self, str: &'dt str) -> NonZero<usize> {
        if let Some(idx) = self.string2idx.get(str) {
            return *idx;
        }
        let idx = NonZero::new(self.strings.len() + 1).unwrap();
        self.strings.push(str);
        self.string2idx.insert(str, idx);
        idx
    }
}
