// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use bumpalo::Bump;
use core::ffi::CStr;
use core::ptr::NonNull;
use core::{fmt, mem};
use fallible_iterator::FallibleIterator;
use fdt::{Error, Fdt, NodeName, StringList};
use smallvec::{smallvec, SmallVec};
use sync::OnceLock;

type Link<T> = Option<NonNull<T>>;

static DEVICE_TREE: OnceLock<DeviceTree> = OnceLock::new();

pub fn device_tree() -> &'static DeviceTree {
    DEVICE_TREE.get().expect("device tree not initialized")
}

/// A device tree describing the hardware configuration of the system.
#[ouroboros::self_referencing] // `root` and all other nodes & data borrows from `alloc`
pub struct DeviceTree {
    alloc: Bump,
    #[borrows(alloc)]
    #[covariant]
    root: NonNull<Device<'this>>,
}

/// Tree of the following shape:
///
///
///                root
///              /
///            /
///          node  -  node  -  node
///        /                 /
///      /                 /
///     node  -  node     node
///
/// where each node has a pointer to its first child, which in turn form a linked list of siblings.
/// additionally each node has a pointer to back its parent.
pub struct Device<'a> {
    /// The name of the device
    pub name: NodeName<'a>,
    pub compatible: &'a str,

    // linked list of device properties
    properties: Link<Property<'a>>,
    // links to other devices in the tree
    parent: Link<Device<'a>>,
    first_child: Link<Device<'a>>,
    next_sibling: Link<Device<'a>>,
}

/// A property of a device.
pub struct Property<'a> {
    inner: fdt::Property<'a>,
    next: Link<Property<'a>>,
}

// Safety: `DeviceTree`s accessor methods allow non-mutable access.
unsafe impl Send for DeviceTree {}
// Safety: `DeviceTree`s accessor methods allow non-mutable access.
unsafe impl Sync for DeviceTree {}

impl fmt::Debug for DeviceTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceTree")
            .field("root", &self.root())
            .finish()
    }
}

impl DeviceTree {
    /// Matches the root device tree `compatible` string against the given list of strings.
    #[inline]
    pub fn is_compatible(&self, compats: &[&str]) -> bool {
        self.root().is_compatible(compats)
    }

    /// Returns an iterator over all top-level devices in the tree.
    #[inline]
    pub fn children(&self) -> Children {
        self.root().children()
    }

    /// Returns an iterator over all nodes in the tree in depth-first order.
    #[inline]
    pub fn descendants(&self) -> Descendants {
        self.root().descendants()
    }

    /// Returns an iterator over all top-level properties in the tree.
    #[inline]
    pub fn properties(&self) -> Properties {
        self.root().properties()
    }

    /// Returns the top-level property with the given name.
    #[inline]
    pub fn property(&self, name: &str) -> Option<&Property> {
        self.root().property(name)
    }

    /// Returns the device with the given path.
    #[inline]
    pub fn find_by_path(&self, path: &str) -> Option<&Device> {
        self.root().find_by_path(path)
    }

    #[inline]
    fn root(&self) -> &Device {
        // Safety: `init` guarantees the root node always exists and is correctly initialized
        unsafe { self.borrow_root().as_ref() }
    }
}

impl fmt::Debug for Device<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let alternate = f.alternate();

        let mut s = f.debug_struct("Device");
        s.field("name", &self.name)
            .field("compatible", &self.compatible);

        if alternate {
            s.field_with("<properties>", |f| {
                let mut f = f.debug_list();
                for prop in self.properties() {
                    f.entry(&prop);
                }
                f.finish()
            });

            s.field_with("<children>", |f| {
                let mut f = f.debug_list();
                for prop in self.children() {
                    f.entry(&prop);
                }
                f.finish()
            });

            s.finish()
        } else {
            s.finish_non_exhaustive()
        }
    }
}

impl<'a> Device<'a> {
    /// Returns `true` if this device is usable, i.e. its reported status property is "okay".
    pub fn is_available(&self) -> bool {
        self.properties()
            .any(|prop| prop.inner.name == "status" && prop.inner.raw == b"okay")
    }

    /// Matches the device `compatible` string against the given list of strings.
    pub fn is_compatible(&self, compats: &[&str]) -> bool {
        compats.iter().any(|&c| c == self.compatible)
    }

    pub fn parent(&self) -> Option<&Device<'a>> {
        // Safety: tree construction guarantees that the pointer is valid
        self.parent.map(|parent| unsafe { parent.as_ref() })
    }

    /// Returns an iterator over all immediate children of this device.
    pub fn children(&self) -> Children {
        Children {
            current: self.first_child,
        }
    }

    /// Returns an iterator over all descendants of this device in depth-first order.
    pub fn descendants(&self) -> Descendants {
        Descendants {
            stack: smallvec![],
            current: self.children(),
        }
    }

    /// Returns an iterator over all properties of this device.
    pub fn properties(&self) -> Properties {
        Properties {
            current: self.properties,
        }
    }

    /// Returns the property with the given name.
    pub fn property(&self, name: &str) -> Option<&Property> {
        self.properties().find(|prop| prop.inner.name == name)
    }

    /// Returns the device with the given path starting from this device.
    pub fn find_by_path(&self, path: &str) -> Option<&Device> {
        let mut node = self;
        for component in path.trim_start_matches('/').split('/') {
            node = node.children().find(|child| child.name.name == component)?;
        }
        Some(node)
    }
}

impl fmt::Debug for Property<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Property")
            .field("name", &self.inner.name)
            .field("raw", &self.inner.raw)
            .finish()
    }
}

impl<'a> Property<'a> {
    /// Returns the property as a `u32`.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a u32.
    pub fn as_u32(&self) -> Result<u32, Error> {
        self.inner.as_u32()
    }

    /// Returns the property as a `u64`.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a u64.
    pub fn as_u64(&self) -> Result<u64, Error> {
        self.inner.as_u64()
    }

    /// Returns the property as a `usize`.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a usize.
    pub fn as_usize(&self) -> Result<usize, Error> {
        self.inner.as_usize()
    }

    /// Returns the property as a C string.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a valid C string.
    pub fn as_cstr(&self) -> Result<&'a CStr, Error> {
        self.inner.as_cstr()
    }

    /// Returns the property as a string.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a valid UTF-8 string.
    pub fn as_str(&self) -> Result<&'a str, Error> {
        self.inner.as_str()
    }

    /// Returns a fallible iterator over the strings in the property.
    ///
    /// # Errors
    ///
    /// Returns an error if the property is not a valid UTF-8 string.
    pub fn as_strlist(&self) -> Result<StringList<'a>, Error> {
        self.inner.as_strlist()
    }
}

pub struct Children<'a> {
    current: Link<Device<'a>>,
}

impl<'a> Iterator for Children<'a> {
    type Item = &'a Device<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // Safety: tree construction guarantees that the pointer is valid
        let dev = unsafe { self.current?.as_ref() };
        self.current = dev.next_sibling;
        Some(dev)
    }
}

pub struct Descendants<'a> {
    stack: SmallVec<[Children<'a>; 6]>,
    current: Children<'a>,
}

impl<'a> Iterator for Descendants<'a> {
    type Item = (usize, &'a Device<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(next) = self.current.next() {
            let depth = self.stack.len();
            if next.first_child.is_some() {
                let parent = mem::replace(&mut self.current, next.children());
                self.stack.push(parent);
            }
            Some((depth, next))
        } else {
            self.current = self.stack.pop()?;
            self.next()
        }
    }
}

pub struct Properties<'a> {
    current: Link<Property<'a>>,
}

impl<'a> Iterator for Properties<'a> {
    type Item = &'a Property<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // Safety: list construction guarantees that the pointer is valid
        let dev = unsafe { self.current?.as_ref() };
        self.current = dev.next;
        Some(dev)
    }
}

#[cold]
pub fn init(fdt: &[u8]) -> crate::Result<&'static DeviceTree> {
    DEVICE_TREE.get_or_try_init(|| {
        // Safety: u32 has no invalid bit patterns
        let (left, aligned, _) = unsafe { fdt.align_to::<u32>() };
        assert!(left.is_empty()); // TODO decide what to do with unaligned slices
        let fdt = Fdt::new(aligned)?;

        let alloc = Bump::new();

        DeviceTree::try_new(alloc, move |alloc| {
            let mut stack: [Link<Device>; 16] = [const { None }; 16];

            let root = unflatten_root(&fdt, alloc)?;
            stack[0] = Some(root);

            let mut iter = fdt.nodes()?;
            while let Some((depth, node)) = iter.next()? {
                let ptr = unflatten_node(node, stack[depth - 1].unwrap(), stack[depth], alloc)?;

                // insert ourselves into the stack so we will become the new previous sibling in the next iteration
                stack[depth] = Some(ptr);
            }

            Ok(root)
        })
    })
}

fn unflatten_root<'a>(fdt: &Fdt, alloc: &'a Bump) -> crate::Result<NonNull<Device<'a>>> {
    let mut compatible: Option<&str> = None;

    let mut props_head: Link<Property> = None;
    let mut props_tail: Link<Property> = None;

    let mut props = fdt.properties();
    while let Some(prop) = props.next()? {
        if prop.name == "compatible" {
            compatible = Some(alloc.alloc_str(prop.as_str()?));
        } else {
            unflatten_property(prop, &mut props_head, &mut props_tail, alloc);
        }
    }

    let ptr = NonNull::from(alloc.alloc(Device {
        name: NodeName {
            name: "",
            unit_address: None,
        },
        compatible: compatible.unwrap_or_default(),
        properties: props_head,
        parent: None,
        first_child: None,
        next_sibling: None,
    }));

    Ok(ptr)
}

fn unflatten_node<'a>(
    node: fdt::Node,
    mut parent: NonNull<Device<'a>>,
    prev_sibling: Link<Device<'a>>,
    alloc: &'a Bump,
) -> crate::Result<NonNull<Device<'a>>> {
    let mut compatible: Option<&'a str> = None;

    let mut props_head: Link<Property> = None;
    let mut props_tail: Link<Property> = None;

    let mut props = node.properties();
    while let Some(prop) = props.next()? {
        if prop.name == "compatible" {
            compatible = Some(alloc.alloc_str(prop.as_str()?));
        } else {
            unflatten_property(prop, &mut props_head, &mut props_tail, alloc);
        }
    }

    let name = node.name()?;
    let node = NonNull::from(alloc.alloc(Device {
        name: NodeName {
            name: alloc.alloc_str(name.name),
            unit_address: name.unit_address.map(|addr| &*alloc.alloc_str(addr)),
        },
        compatible: compatible.unwrap_or_default(),
        properties: props_head,
        parent: Some(parent),
        first_child: None,
        next_sibling: None,
    }));

    // update the parents `first_child` pointer if necessary
    // Safety: callers responsibility to ensure that the parent pointer is valid
    unsafe {
        parent.as_mut().first_child.get_or_insert(node);
    }

    // update the previous sibling's `next_sibling` pointer if necessary
    if let Some(mut sibling) = prev_sibling {
        // Safety: callers responsibility to ensure that the parent pointer is valid
        unsafe {
            sibling.as_mut().next_sibling = Some(node);
        }
    }

    Ok(node)
}

fn unflatten_property<'a>(
    prop: fdt::Property,
    head: &mut Link<Property<'a>>,
    tail: &mut Link<Property<'a>>,
    alloc: &'a Bump,
) {
    let prop = NonNull::from(alloc.alloc(Property {
        inner: fdt::Property {
            name: alloc.alloc_str(prop.name),
            raw: alloc.alloc_slice_copy(prop.raw),
        },
        next: None,
    }));

    // if there already is a tail node append the new node to it
    if let &mut Some(mut tail) = tail {
        // Safety: tail is either `None` or a valid pointer we allocated in a previous call below
        let tail = unsafe { tail.as_mut() };
        tail.next = Some(prop);
    } else {
        // otherwise the list is empty, so update the head pointer
        debug_assert!(head.is_none());
        *head = Some(prop);
    }

    // update the tail pointer so we will become the new tail in the next iteration
    *tail = Some(prop);
}
