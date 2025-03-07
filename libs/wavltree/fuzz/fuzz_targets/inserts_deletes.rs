// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::fmt;
use std::mem::offset_of;
use std::pin::Pin;
use std::ptr::NonNull;
use wavltree::Linked;
use wavltree::{Links, WAVLTree};

#[derive(Default)]
struct TestEntry {
    value: usize,
    links: Links<Self>,
}
impl TestEntry {
    pub fn new(value: usize) -> Self {
        let mut this = Self::default();
        this.value = value;
        this
    }
}
impl fmt::Debug for TestEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlaceHolderEntry")
            .field("value", &self.value)
            .finish()
    }
}
unsafe impl Linked for TestEntry {
    type Handle = Pin<Box<Self>>;
    type Key = usize;
    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        unsafe { NonNull::from(Box::leak(Pin::into_inner_unchecked(handle))) }
    }
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // Safety: `NonNull` *must* be constructed from a pinned reference
        // which the tree implementation upholds.
        Pin::new_unchecked(Box::from_raw(ptr.as_ptr()))
    }
    unsafe fn links(target: NonNull<Self>) -> NonNull<Links<TestEntry>> {
        target
            .map_addr(|addr| {
                let offset = offset_of!(Self, links);
                addr.checked_add(offset).unwrap()
            })
            .cast()
    }
    fn get_key(&self) -> &Self::Key {
        &self.value
    }
}

fuzz_target!(|inserts_removals: (Vec<usize>, Vec<usize>)| {
    let mut tree: WAVLTree<TestEntry> = WAVLTree::new();

    for i in inserts_removals.0 {
        tree.insert(Box::pin(TestEntry::new(i)));
        tree.assert_valid();
    }

    for i in inserts_removals.1 {
        tree.insert(Box::pin(TestEntry::new(i)));
        tree.assert_valid();
    }
});
