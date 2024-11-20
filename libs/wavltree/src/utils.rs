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
    pub(crate) fn opposite(&self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

pub unsafe fn get_sibling<T>(node: Link<T>, parent: NonNull<T>) -> (Link<T>, Side)
where
    T: Linked + ?Sized,
{
    if let Some(node) = node {
        debug_assert_eq!(
            T::links(node).as_ref().parent(),
            Some(parent),
            "node {parent:#?} is not a parent of {node:#?}"
        );
    }

    let parent_lks = T::links(parent).as_ref();
    if parent_lks.left() == node {
        (parent_lks.right(), Side::Right)
    } else {
        (parent_lks.left(), Side::Left)
    }
}

pub unsafe fn get_link_parity<T: Linked + ?Sized>(p_x: Link<T>) -> bool {
    if let Some(p_x) = p_x {
        T::links(p_x).as_ref().rank_parity()
    } else {
        // `None` means "missing node" which has rank -1 and therefore parity 1
        true
    }
}

/// Returns whether the given `node` is a 2-child of `parent` ie whether the rank-difference
/// between `node` and `parent` is 2.
pub unsafe fn node_is_2_child<T: Linked + ?Sized>(node: NonNull<T>, parent: NonNull<T>) -> bool {
    let node_links = T::links(node).as_ref();
    let parent_links = T::links(parent).as_ref();

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

pub unsafe fn find_minimum<T: Linked + ?Sized>(mut curr: NonNull<T>) -> NonNull<T> {
    while let Some(left) = T::links(curr).as_ref().left() {
        curr = left;
    }

    curr
}

pub unsafe fn find_maximum<T: Linked + ?Sized>(mut curr: NonNull<T>) -> NonNull<T> {
    while let Some(right) = T::links(curr).as_ref().right() {
        curr = right;
    }

    curr
}
