// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::boxed::Box;
use core::alloc::Layout;
use core::mem::offset_of;
use core::ops::Range;
use core::pin::Pin;
use core::ptr::NonNull;

use pin_project::pin_project;

use crate::{AccessRules, VirtualAddress};

#[pin_project(!Unpin)]
#[derive(Debug)]
pub struct AddressSpaceRegion {
    range: Range<VirtualAddress>,
    access_rules: AccessRules,
    #[cfg(debug_assertions)]
    layout: Layout,

    /// The address range covered by this region and its WAVL tree subtree, used when allocating new regions
    subtree_range: Range<VirtualAddress>,
    /// The largest gap in this subtree, used when allocating new regions
    max_gap: usize,

    /// Links to other regions in the WAVL tree
    #[pin]
    links: wavltree::Links<AddressSpaceRegion>,
}

// Safety: the pinning and !Unpin requirements are enforced by the `#[pin_project(!Unpin)]` attribute
// of the `AddressSpaceRegion`. see above.
unsafe impl wavltree::Linked for AddressSpaceRegion {
    /// Any heap-allocated type that owns an element may be used.
    ///
    /// An element *must not* move while part of an intrusive data
    /// structure. In many cases, `Pin` may be used to enforce this.
    type Handle = Pin<Box<Self>>; // TODO better handle type

    type Key = VirtualAddress;

    /// Convert an owned `Handle` into a raw pointer
    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        // Safety: wavltree treats the ptr as pinned
        unsafe { NonNull::from(Box::leak(Pin::into_inner_unchecked(handle))) }
    }

    /// Convert a raw pointer back into an owned `Handle`.
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // Safety: `NonNull` *must* be constructed from a pinned reference
        // which the tree implementation upholds.
        unsafe { Pin::new_unchecked(Box::from_raw(ptr.as_ptr())) }
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<wavltree::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }

    fn get_key(&self) -> &Self::Key {
        &self.range.start
    }
}
