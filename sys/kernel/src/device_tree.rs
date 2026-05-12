// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::ffi::CStr;
use core::num::NonZeroU32;
use core::ptr::NonNull;
use core::{fmt, iter, mem, slice};

use bumpalo::Bump;
use bumpalo::collections::Vec as BumpVec;
use fallible_iterator::FallibleIterator;
use fdt::{CellSizes, Error, Fdt, NodeName, StringList};
use smallvec::{SmallVec, smallvec};

/// Handle to a [`Device`] record in a [`DeviceTree`]. `NonZeroU32` so that
/// `Option<DeviceId>` is the same size as `DeviceId`.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct DeviceId(NonZeroU32);

impl DeviceId {
    fn idx(self) -> usize {
        self.0.get() as usize - 1
    }
}

/// Handle to a [`Property`] record in a [`DeviceTree`]. See [`DeviceId`].
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct PropertyId(NonZeroU32);

impl PropertyId {
    fn idx(self) -> usize {
        self.0.get() as usize - 1
    }
}

/// Raw `(ptr, len)` view into a UTF-8 slice the [`Builder`] allocated in the
/// arena's bump. Stored without a lifetime so that [`DeviceNode`] /
/// [`PropertyNode`] don't carry one — the lifetime is reattached at access
/// time by [`Arena::device`] / [`Arena::property`].
#[derive(Copy, Clone)]
struct BumpStr {
    ptr: NonNull<u8>,
    len: usize,
}

impl BumpStr {
    /// Reattach a lifetime to the underlying bytes and return them as a `&str`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee:
    ///
    /// 1. `self.ptr` is valid for reads of `self.len` consecutive bytes
    ///    throughout `'a`, and those bytes form valid UTF-8.
    /// 2. No `&mut` reference overlaps with that region for the duration of `'a`.
    unsafe fn as_str<'a>(self) -> &'a str {
        // Safety: ensured by caller.
        unsafe {
            core::str::from_utf8_unchecked(slice::from_raw_parts(self.ptr.as_ptr(), self.len))
        }
    }
}

/// As [`BumpStr`], for an arbitrary byte slice.
#[derive(Copy, Clone)]
struct BumpBytes {
    ptr: NonNull<u8>,
    len: usize,
}

impl BumpBytes {
    /// Reattach a lifetime to the underlying bytes and return them as a `&[u8]`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee:
    ///
    /// 1. `self.ptr` is valid for reads of `self.len` consecutive initialised
    ///    bytes throughout `'a`.
    /// 2. No `&mut` reference overlaps with that region for the duration of `'a`.
    unsafe fn as_slice<'a>(self) -> &'a [u8] {
        // Safety: ensured by caller.
        unsafe { slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

/// Owned node record stored in the arena.
struct DeviceNode {
    name: BumpStr,
    unit_address: Option<BumpStr>,
    compatible: BumpStr,
    phandle: Option<u32>,
    properties: Option<PropertyId>,
    parent: Option<DeviceId>,
    first_child: Option<DeviceId>,
    next_sibling: Option<DeviceId>,
}

/// Owned property record stored in the arena.
struct PropertyNode {
    name: BumpStr,
    raw: BumpBytes,
    next: Option<PropertyId>,
}

/// Backing storage for a parsed device tree: a bump allocator that owns the raw
/// bytes of every string and slice. All `BumpStr`s and `BumpBytes` appearing in
/// `devices` and `properties` are guaranteed to be backed by `bump`.
///
/// It is critical for safety that this struct remains immutable after its
/// creation.
struct Arena {
    bump: Bump,
    // Raw fat pointers into bump-allocated slices, finalized by
    // `Builder::finish`. Wrapped as `NonNull<[T]>` so the `Arena` itself
    // has no `'bump` lifetime — which is what lets `DeviceTree` hold the
    // arena by value.
    devices: NonNull<[DeviceNode]>,
    properties: NonNull<[PropertyNode]>,
    // Sorted ascending by phandle.
    phandle_index: NonNull<[(u32, DeviceId)]>,
}

impl Arena {
    /// Reconstruct a [`Device`] view from a [`DeviceId`]. The returned value
    /// borrows the strings it exposes from `self`'s bump.
    fn device(&self, id: DeviceId) -> Device<'_> {
        // Safety: `self.devices` was constructed by `Builder::finish` from a
        // `BumpVec` finalized via `into_bump_slice`; its bytes live in
        // `self.bump`, which is alive while `&self` is held. The slice is
        // never mutated after construction. Every `BumpStr` in a
        // `DeviceNode` was produced by `Builder::alloc_str`, so it points
        // at a bump-allocated valid UTF-8 slice.
        let n = &unsafe { self.devices.as_ref() }[id.idx()];
        Device {
            name: NodeName {
                name: unsafe { n.name.as_str() },
                unit_address: n.unit_address.map(|s| unsafe { s.as_str() }),
            },
            compatible: unsafe { n.compatible.as_str() },
            phandle: n.phandle,
            properties: n.properties,
            parent: n.parent,
            first_child: n.first_child,
            next_sibling: n.next_sibling,
        }
    }

    fn property(&self, id: PropertyId) -> Property<'_> {
        // Safety: see `Arena::device`.
        let p = &unsafe { self.properties.as_ref() }[id.idx()];
        Property {
            name: unsafe { p.name.as_str() },
            raw: unsafe { p.raw.as_slice() },
            next: p.next,
        }
    }

    fn find_phandle(&self, phandle: u32) -> Option<DeviceId> {
        // Safety: see `Arena::device`.
        let idx = unsafe { self.phandle_index.as_ref() };
        idx.binary_search_by_key(&phandle, |&(p, _)| p)
            .ok()
            .map(|i| idx[i].1)
    }
}

/// A device tree describing the hardware configuration of the system.
pub struct DeviceTree {
    arena: Arena,
    root: DeviceId,
}

// Safety: `DeviceTree`'s accessor methods only hand out shared references.
// The arena's three `NonNull<[T]>` slice pointers and the `Bump` they reference
// are mutated only during `parse`, before the `DeviceTree` is shared.
unsafe impl Send for DeviceTree {}
// Safety: see `Send`.
unsafe impl Sync for DeviceTree {}

impl fmt::Debug for DeviceTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceTree")
            .field("root", &self.root())
            .finish()
    }
}

impl DeviceTree {
    /// Parse the given flattened device tree blob.
    pub fn parse(fdt: &[u8]) -> crate::Result<Self> {
        // Safety: u32 has no invalid bit patterns
        let (left, aligned, _) = unsafe { fdt.align_to::<u32>() };
        assert!(left.is_empty()); // TODO decide what to do with unaligned slices
        let fdt = Fdt::new(aligned)?;

        let bump = Bump::new();
        let mut b = Builder::new(&bump);

        let root = unflatten_root(&fdt, &mut b)?;
        let mut stack: [Option<DeviceId>; 16] = [None; 16];
        stack[0] = Some(root);

        let mut iter = fdt.nodes()?;
        while let Some((depth, node)) = iter.next()? {
            let id = unflatten_node(node, stack[depth - 1].unwrap(), stack[depth], &mut b)?;
            stack[depth] = Some(id);
        }

        let (devices, properties, phandle_index) = b.finish();

        Ok(Self {
            arena: Arena {
                bump,
                devices,
                properties,
                phandle_index,
            },
            root,
        })
    }

    /// The root device tree node.
    #[inline]
    pub fn root(&self) -> Device<'_> {
        self.arena.device(self.root)
    }

    /// Matches the root device tree `compatible` string against the given list.
    #[inline]
    pub fn is_compatible<'b>(&self, compats: impl IntoIterator<Item = &'b str>) -> bool {
        self.root().is_compatible(compats)
    }

    /// Iterator over all top-level devices in the tree.
    #[inline]
    pub fn children(&self) -> Children<'_> {
        Children::new(self, self.root().first_child)
    }

    /// Iterator over all nodes in the tree in depth-first order.
    #[inline]
    pub fn descendants(&self) -> Descendants<'_> {
        Descendants::new(self.children())
    }

    /// Iterator over all top-level properties in the tree.
    #[inline]
    pub fn properties(&self) -> Properties<'_> {
        Properties::new(self, self.root().properties)
    }

    /// Returns the top-level property with the given name.
    #[inline]
    pub fn property(&self, name: &str) -> Option<Property<'_>> {
        self.root().property(self, name)
    }

    /// Returns the device with the given path.
    #[inline]
    pub fn find_by_path(&self, path: &str) -> Option<Device<'_>> {
        self.root().find_by_path(self, path)
    }

    /// Returns the device with the given phandle.
    pub fn find_by_phandle(&self, phandle: u32) -> Option<Device<'_>> {
        Some(self.arena.device(self.arena.find_phandle(phandle)?))
    }
}

/// A node in the device tree.
///
/// Holds the immutable per-node data inline as borrowed slices and stores
/// parent/child/sibling links as IDs that resolve through a [`DeviceTree`].
#[derive(Copy, Clone)]
pub struct Device<'arena> {
    /// The name of this device (node name + optional unit address).
    pub name: NodeName<'arena>,
    /// The contents of the `compatible` property, or `""` if absent.
    pub compatible: &'arena str,
    /// The `phandle` property, if present.
    pub phandle: Option<u32>,

    properties: Option<PropertyId>,
    parent: Option<DeviceId>,
    first_child: Option<DeviceId>,
    next_sibling: Option<DeviceId>,
}

impl fmt::Debug for Device<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Device")
            .field("name", &self.name)
            .field("compatible", &self.compatible)
            .field("phandle", &self.phandle)
            .finish_non_exhaustive()
    }
}

impl<'arena> Device<'arena> {
    /// Matches the device `compatible` string against the given list.
    pub fn is_compatible<'b>(&self, compats: impl IntoIterator<Item = &'b str>) -> bool {
        compats.into_iter().any(|c| self.compatible.contains(c))
    }

    /// This device's parent, if any.
    pub fn parent(&self, devtree: &'arena DeviceTree) -> Option<Device<'arena>> {
        Some(devtree.arena.device(self.parent?))
    }

    /// Iterator over all immediate children.
    pub fn children(&self, devtree: &'arena DeviceTree) -> Children<'arena> {
        Children::new(devtree, self.first_child)
    }

    /// Iterator over all descendants in depth-first order.
    pub fn descendants(&self, devtree: &'arena DeviceTree) -> Descendants<'arena> {
        Descendants::new(self.children(devtree))
    }

    /// Iterator over all properties of this device.
    pub fn properties(&self, devtree: &'arena DeviceTree) -> Properties<'arena> {
        Properties::new(devtree, self.properties)
    }

    /// Property with the given name, if any.
    pub fn property(&self, devtree: &'arena DeviceTree, name: &str) -> Option<Property<'arena>> {
        self.properties(devtree).find(|p| p.name == name)
    }

    /// Returns the device with the given path starting from this device.
    pub fn find_by_path(self, devtree: &'arena DeviceTree, path: &str) -> Option<Device<'arena>> {
        let mut node = self;
        for component in path.trim_start_matches('/').split('/') {
            node = node
                .children(devtree)
                .find(|child| child.name.name == component)?;
        }
        Some(node)
    }

    /// Effective `#address-cells`/`#size-cells` at this node, inheriting
    /// from the closest ancestor that declares them.
    pub fn cell_sizes(&self, devtree: &'arena DeviceTree) -> CellSizes {
        let address_cells = self
            .property(devtree, "#address-cells")
            .and_then(|prop| prop.as_usize().ok());
        let size_cells = self
            .property(devtree, "#size-cells")
            .and_then(|prop| prop.as_usize().ok());

        if let (Some(address_cells), Some(size_cells)) = (address_cells, size_cells) {
            CellSizes {
                address_cells,
                size_cells,
            }
        } else if let Some(parent) = self.parent(devtree) {
            parent.cell_sizes(devtree)
        } else {
            CellSizes::default()
        }
    }

    /// Decoded `reg` property iterator, using the inherited cell sizes.
    pub fn regs(&self, devtree: &'arena DeviceTree) -> Option<fdt::Regs<'arena>> {
        let prop = self.property(devtree, "reg")?;
        Some(prop.as_fdt_property().as_regs(self.cell_sizes(devtree)))
    }

    /// `#interrupt-cells` declared on this node, if any.
    pub fn interrupt_cells(&self, devtree: &'arena DeviceTree) -> Option<usize> {
        self.property(devtree, "#interrupt-cells")?.as_usize().ok()
    }

    /// Resolve the `interrupt-parent` property through `find_by_phandle`.
    pub fn interrupt_parent(&self, devtree: &'arena DeviceTree) -> Option<Device<'arena>> {
        let phandle = self.property(devtree, "interrupt-parent")?.as_u32().ok()?;
        devtree.find_by_phandle(phandle)
    }

    /// Iterate the `interrupts` property as `(parent, IrqSource)` pairs.
    pub fn interrupts(self, devtree: &'arena DeviceTree) -> Option<Interrupts<'arena>> {
        let prop = self.property(devtree, "interrupts")?;
        let parent = self.interrupt_parent(devtree)?;
        Some(Interrupts {
            parent,
            parent_cells: parent.interrupt_cells(devtree)?,
            raw: prop.raw.array_chunks::<4>().map(chunk_to_u32),
        })
    }

    /// Iterate the `interrupts-extended` property as `(parent, IrqSource)` pairs.
    pub fn interrupts_extended(
        self,
        devtree: &'arena DeviceTree,
    ) -> Option<InterruptsExtended<'arena>> {
        let prop = self.property(devtree, "interrupts-extended")?;
        Some(InterruptsExtended {
            devtree,
            raw: prop.raw.array_chunks::<4>().map(chunk_to_u32),
        })
    }
}

/// A property of a device.
#[derive(Copy, Clone)]
pub struct Property<'arena> {
    /// The property name.
    pub name: &'arena str,
    /// The raw property bytes.
    pub raw: &'arena [u8],

    next: Option<PropertyId>,
}

impl fmt::Debug for Property<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Property")
            .field("name", &self.name)
            .field("raw", &self.raw)
            .finish()
    }
}

impl<'arena> Property<'arena> {
    fn as_fdt_property(&self) -> fdt::Property<'arena> {
        fdt::Property {
            name: self.name,
            raw: self.raw,
        }
    }

    /// Returns the property as a `u32`.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a u32.
    pub fn as_u32(&self) -> Result<u32, Error> {
        self.as_fdt_property().as_u32()
    }

    /// Returns the property as a `u64`.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a u64.
    pub fn as_u64(&self) -> Result<u64, Error> {
        self.as_fdt_property().as_u64()
    }

    /// Returns the property as a `usize`.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a usize.
    pub fn as_usize(&self) -> Result<usize, Error> {
        self.as_fdt_property().as_usize()
    }

    /// Returns the property as a C string.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a valid C string.
    pub fn as_cstr(&self) -> Result<&'arena CStr, Error> {
        self.as_fdt_property().as_cstr()
    }

    /// Returns the property as a string.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a valid UTF-8 string.
    pub fn as_str(&self) -> Result<&'arena str, Error> {
        self.as_fdt_property().as_str()
    }

    /// Returns a fallible iterator over the strings in the property.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a valid UTF-8 string.
    pub fn as_strlist(&self) -> Result<StringList<'arena>, Error> {
        self.as_fdt_property().as_strlist()
    }
}

/// Iterator over an immediate-children list of a device.
pub struct Children<'a> {
    devtree: &'a DeviceTree,
    current: Option<DeviceId>,
}

impl<'a> Children<'a> {
    fn new(devtree: &'a DeviceTree, head: Option<DeviceId>) -> Self {
        Self {
            devtree,
            current: head,
        }
    }
}

impl<'a> Iterator for Children<'a> {
    type Item = Device<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.current?;
        let dev = self.devtree.arena.device(id);
        self.current = dev.next_sibling;
        Some(dev)
    }
}

/// Depth-first iterator over a device's descendants, yielding `(depth, device)`.
pub struct Descendants<'a> {
    stack: SmallVec<[Children<'a>; 6]>,
    current: Children<'a>,
}

impl<'a> Descendants<'a> {
    fn new(children: Children<'a>) -> Self {
        Self {
            stack: smallvec![],
            current: children,
        }
    }
}

impl<'a> Iterator for Descendants<'a> {
    type Item = (usize, Device<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(next) = self.current.next() {
            let depth = self.stack.len();
            if next.first_child.is_some() {
                let devtree = self.current.devtree;
                let parent =
                    mem::replace(&mut self.current, Children::new(devtree, next.first_child));
                self.stack.push(parent);
            }
            Some((depth, next))
        } else {
            self.current = self.stack.pop()?;
            self.next()
        }
    }
}

/// Iterator over a device's property list.
pub struct Properties<'a> {
    devtree: &'a DeviceTree,
    current: Option<PropertyId>,
}

impl<'a> Properties<'a> {
    fn new(devtree: &'a DeviceTree, head: Option<PropertyId>) -> Self {
        Self {
            devtree,
            current: head,
        }
    }
}

impl<'a> Iterator for Properties<'a> {
    type Item = Property<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let id = self.current?;
        let prop = self.devtree.arena.property(id);
        self.current = prop.next;
        Some(prop)
    }
}

#[derive(Debug)]
pub enum IrqSource {
    C1(u32),
    C3(u32, u32, u32),
}

#[expect(clippy::type_complexity, reason = "this is not thaaat complex")]
pub struct Interrupts<'a> {
    parent: Device<'a>,
    parent_cells: usize,
    raw: iter::Map<slice::ArrayChunks<'a, u8, 4>, fn(&[u8; 4]) -> u32>,
}
impl<'a> Iterator for Interrupts<'a> {
    type Item = (Device<'a>, IrqSource);

    fn next(&mut self) -> Option<Self::Item> {
        Some((
            self.parent,
            interrupt_address(&mut self.raw, self.parent_cells)?,
        ))
    }
}

#[expect(clippy::type_complexity, reason = "this is not thaaat complex")]
pub struct InterruptsExtended<'a> {
    devtree: &'a DeviceTree,
    raw: iter::Map<slice::ArrayChunks<'a, u8, 4>, fn(&[u8; 4]) -> u32>,
}
impl<'a> Iterator for InterruptsExtended<'a> {
    type Item = (Device<'a>, IrqSource);

    fn next(&mut self) -> Option<Self::Item> {
        let parent_phandle = self.raw.next()?;
        let parent = self.devtree.find_by_phandle(parent_phandle)?;
        let parent_interrupt_cells = parent.interrupt_cells(self.devtree)?;
        Some((
            parent,
            interrupt_address(&mut self.raw, parent_interrupt_cells)?,
        ))
    }
}

fn chunk_to_u32(chunk: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*chunk)
}

fn interrupt_address(
    iter: &mut impl Iterator<Item = u32>,
    interrupt_cells: usize,
) -> Option<IrqSource> {
    match interrupt_cells {
        1 => Some(IrqSource::C1(iter.next()?)),
        3 if let Ok([a, b, c]) = iter.next_chunk() => Some(IrqSource::C3(a, b, c)),
        _ => None,
    }
}

// ===== construction =====

/// Scoped builder that owns the in-flight `BumpVec`s. After `parse` finishes
/// pushing nodes/properties, [`Builder::finish`] converts each vec to a
/// `&'bump [T]` slice via `into_bump_slice` and erases the `'bump` lifetime
/// to `NonNull<[T]>` so the resulting `Arena` has no lifetime parameter.
struct Builder<'bump> {
    bump: &'bump Bump,
    devices: BumpVec<'bump, DeviceNode>,
    properties: BumpVec<'bump, PropertyNode>,
    phandle_index: BumpVec<'bump, (u32, DeviceId)>,
}

impl<'bump> Builder<'bump> {
    fn new(bump: &'bump Bump) -> Self {
        Self {
            bump,
            devices: BumpVec::new_in(bump),
            properties: BumpVec::new_in(bump),
            phandle_index: BumpVec::new_in(bump),
        }
    }

    fn alloc_str(&self, s: &str) -> BumpStr {
        let s: &mut str = self.bump.alloc_str(s);
        let len = s.len();
        let ptr = NonNull::from(s.as_bytes()).cast::<u8>();
        BumpStr { ptr, len }
    }

    fn alloc_bytes(&self, b: &[u8]) -> BumpBytes {
        let b: &mut [u8] = self.bump.alloc_slice_copy(b);
        let len = b.len();
        let ptr = NonNull::from(&*b).cast::<u8>();
        BumpBytes { ptr, len }
    }

    fn push_device(&mut self, n: DeviceNode) -> DeviceId {
        self.devices.push(n);
        DeviceId(NonZeroU32::new(self.devices.len() as u32).unwrap())
    }

    fn push_property(&mut self, p: PropertyNode) -> PropertyId {
        self.properties.push(p);
        PropertyId(NonZeroU32::new(self.properties.len() as u32).unwrap())
    }

    fn register_phandle(&mut self, phandle: u32, id: DeviceId) {
        self.phandle_index.push((phandle, id));
    }

    /// Consume the builder, sort the phandle index, and erase the
    /// `'bump` lifetime on the backing slices.
    fn finish(
        self,
    ) -> (
        NonNull<[DeviceNode]>,
        NonNull<[PropertyNode]>,
        NonNull<[(u32, DeviceId)]>,
    ) {
        let mut phandle_index = self.phandle_index;
        phandle_index.sort_unstable_by_key(|&(p, _)| p);
        let d = NonNull::from(self.devices.into_bump_slice());
        let p = NonNull::from(self.properties.into_bump_slice());
        let h = NonNull::from(phandle_index.into_bump_slice());
        (d, p, h)
    }
}

fn unflatten_root(fdt: &Fdt, b: &mut Builder<'_>) -> crate::Result<DeviceId> {
    let mut compatible: Option<BumpStr> = None;

    let mut props_head: Option<PropertyId> = None;
    let mut props_tail: Option<PropertyId> = None;

    let mut props = fdt.properties();
    while let Some(prop) = props.next()? {
        if prop.name == "compatible" {
            compatible = Some(b.alloc_str(prop.as_str()?));
        } else {
            unflatten_property(prop, &mut props_head, &mut props_tail, b);
        }
    }

    let empty = b.alloc_str("");
    Ok(b.push_device(DeviceNode {
        name: empty,
        unit_address: None,
        compatible: compatible.unwrap_or(empty),
        phandle: None,
        properties: props_head,
        parent: None,
        first_child: None,
        next_sibling: None,
    }))
}

fn unflatten_node(
    node: fdt::Node,
    parent: DeviceId,
    prev_sibling: Option<DeviceId>,
    b: &mut Builder<'_>,
) -> crate::Result<DeviceId> {
    let mut compatible: Option<BumpStr> = None;
    let mut phandle: Option<u32> = None;

    let mut props_head: Option<PropertyId> = None;
    let mut props_tail: Option<PropertyId> = None;

    let mut props = node.properties();
    while let Some(prop) = props.next()? {
        if prop.name == "compatible" {
            compatible = Some(b.alloc_str(prop.as_str()?));
        } else if prop.name == "phandle" {
            phandle = prop.as_u32().ok();
        } else {
            unflatten_property(prop, &mut props_head, &mut props_tail, b);
        }
    }

    let name = node.name()?;
    let name_str = b.alloc_str(name.name);
    let unit_address = name.unit_address.map(|addr| b.alloc_str(addr));
    let compatible = compatible.unwrap_or_else(|| b.alloc_str(""));

    let id = b.push_device(DeviceNode {
        name: name_str,
        unit_address,
        compatible,
        phandle,
        properties: props_head,
        parent: Some(parent),
        first_child: None,
        next_sibling: None,
    });

    if let Some(phandle) = phandle {
        b.register_phandle(phandle, id);
    }

    // Splice into the parent's first_child slot if it's still empty.
    if b.devices[parent.idx()].first_child.is_none() {
        b.devices[parent.idx()].first_child = Some(id);
    }

    // Append to the previous sibling, if any.
    if let Some(prev_id) = prev_sibling {
        b.devices[prev_id.idx()].next_sibling = Some(id);
    }

    Ok(id)
}

fn unflatten_property(
    prop: fdt::Property,
    head: &mut Option<PropertyId>,
    tail: &mut Option<PropertyId>,
    b: &mut Builder<'_>,
) {
    let name = b.alloc_str(prop.name);
    let raw = b.alloc_bytes(prop.raw);
    let id = b.push_property(PropertyNode {
        name,
        raw,
        next: None,
    });

    if let Some(tail_id) = *tail {
        b.properties[tail_id.idx()].next = Some(id);
    } else {
        debug_assert!(head.is_none());
        *head = Some(id);
    }

    *tail = Some(id);
}
