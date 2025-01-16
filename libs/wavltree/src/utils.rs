// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::{Link, Linked};
use core::ptr::NonNull;
use core::{fmt, ptr};

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Side {
    Left,
    Right,
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::Left => f.write_str("left"),
            Side::Right => f.write_str("right"),
        }
    }
}

impl Side {
    pub(crate) fn opposite(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

pub fn get_sibling<T>(node: Link<T>, parent: NonNull<T>) -> (Link<T>, Side)
where
    T: Linked + ?Sized,
{
    if let Some(node) = node {
        debug_assert_eq!(
            unsafe { T::links(node).as_ref() }.parent(),
            Some(parent),
            "node {parent:#?} is not a parent of {node:#?}"
        );
    }

    let parent_lks = unsafe { T::links(parent).as_ref() };
    if parent_lks.left() == node {
        (parent_lks.right(), Side::Right)
    } else {
        (parent_lks.left(), Side::Left)
    }
}

pub fn get_link_parity<T: Linked + ?Sized>(p_x: Link<T>) -> bool {
    if let Some(p_x) = p_x {
        unsafe { T::links(p_x).as_ref() }.rank_parity()
    } else {
        // `None` means "missing node" which has rank -1 and therefore parity 1
        true
    }
}

/// Returns whether the given `node` is a 2-child of `parent` ie whether the rank-difference
/// between `node` and `parent` is 2.
pub fn node_is_2_child<T: Linked + ?Sized>(node: NonNull<T>, parent: NonNull<T>) -> bool {
    let node_links = unsafe { T::links(node).as_ref() };
    let parent_links = unsafe { T::links(parent).as_ref() };

    // do a bit of sanity checking
    debug_assert!(!parent_links.is_leaf(), "parent must be non-leaf");
    debug_assert!(
        parent_links
            .left()
            .is_some_and(|l| ptr::addr_eq(l.as_ptr(), node.as_ptr()))
            || parent_links
                .right()
                .is_some_and(|r| ptr::addr_eq(r.as_ptr(), node.as_ptr())),
        "parent must be parent of node"
    );

    node_links.rank_parity() == parent_links.rank_parity()
}

pub fn find_minimum<T: Linked + ?Sized>(mut curr: NonNull<T>) -> NonNull<T> {
    while let Some(left) = unsafe { T::links(curr).as_ref() }.left() {
        curr = left;
    }

    curr
}

pub fn find_maximum<T: Linked + ?Sized>(mut curr: NonNull<T>) -> NonNull<T> {
    while let Some(right) = unsafe { T::links(curr).as_ref() }.right() {
        curr = right;
    }

    curr
}

pub(crate) fn next<T>(node: NonNull<T>) -> Link<T>
where
    T: Linked + ?Sized,
{
    let node_links = unsafe { T::links(node).as_ref() };

    // If we have a right child, its least descendant is our next node
    if let Some(right) = node_links.right() {
        Some(find_minimum(right))
    } else {
        let mut curr = node;

        loop {
            if let Some(parent) = unsafe { T::links(curr).as_ref() }.parent() {
                let parent_links = unsafe { T::links(parent).as_ref() };

                // if we have a parent, and we're not their right/greater child, that parent is our
                // next node
                if parent_links.right() != Some(curr) {
                    return Some(parent);
                }

                curr = parent;
            } else {
                // we reached the tree root without finding a next node
                return None;
            }
        }
    }
}

pub(crate) unsafe fn prev<T>(node: NonNull<T>) -> Link<T>
where
    T: Linked + ?Sized,
{
    let node_links = unsafe { T::links(node).as_ref() };

    // If we have a left child, its greatest descendant is our previous node
    if let Some(left) = node_links.left() {
        Some(find_maximum(left))
    } else {
        let mut curr = node;

        loop {
            if let Some(parent) = unsafe { T::links(curr).as_ref() }.parent() {
                let parent_links = unsafe { T::links(parent).as_ref() };

                // if we have a parent, and we're not their left/lesser child, that parent is our
                // previous node
                if parent_links.left() != Some(curr) {
                    return Some(parent);
                }

                curr = parent;
            } else {
                // we reached the tree root without finding a previous node
                return None;
            }
        }
    }
}
