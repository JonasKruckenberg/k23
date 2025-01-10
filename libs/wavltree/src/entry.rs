// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::utils::Side;
use crate::{utils, Link, Linked, WAVLTree};
use core::pin::Pin;
use core::ptr::NonNull;

pub enum Entry<'a, T>
where
    T: Linked + ?Sized,
{
    Occupied(OccupiedEntry<'a, T>),
    Vacant(VacantEntry<'a, T>),
}

impl<'a, T> Entry<'a, T>
where
    T: Linked + ?Sized,
{
    pub fn or_insert_with<F>(self, default: F) -> Pin<&'a mut T>
    where
        F: FnOnce() -> T::Handle,
    {
        match self {
            Entry::Occupied(mut entry) => unsafe { Pin::new_unchecked(entry.node.as_mut()) },
            Entry::Vacant(entry) => entry.insert(default()),
        }
    }

    pub fn peek_next(&self) -> Option<&T> {
        match self {
            Entry::Occupied(e) => e.peek_next(),
            Entry::Vacant(e) => e.peek_next(),
        }
    }
    pub fn peek_prev(&self) -> Option<&T> {
        match self {
            Entry::Occupied(e) => e.peek_prev(),
            Entry::Vacant(e) => e.peek_prev(),
        }
    }
    pub fn peek_next_mut(&mut self) -> Option<Pin<&mut T>> {
        match self {
            Entry::Occupied(e) => e.peek_next_mut(),
            Entry::Vacant(e) => e.peek_next_mut(),
        }
    }
    pub fn peek_prev_mut(&mut self) -> Option<Pin<&mut T>> {
        match self {
            Entry::Occupied(e) => e.peek_prev_mut(),
            Entry::Vacant(e) => e.peek_prev_mut(),
        }
    }
}

pub struct OccupiedEntry<'a, T>
where
    T: Linked + ?Sized,
{
    pub(crate) node: NonNull<T>,
    pub(crate) _tree: &'a mut WAVLTree<T>,
}
impl<T> OccupiedEntry<'_, T>
where
    T: Linked + ?Sized,
{
    pub fn get(&self) -> &T {
        unsafe { self.node.as_ref() }
    }
    pub fn get_mut(&mut self) -> Pin<&mut T> {
        unsafe { Pin::new_unchecked(self.node.as_mut()) }
    }
    pub fn remove(self) -> T::Handle {
        self._tree.remove_internal(self.node)
    }
    pub fn peek_next(&self) -> Option<&T> {
        let node = utils::next(self.node)?;
        unsafe { Some(node.as_ref()) }
    }
    pub fn peek_prev(&self) -> Option<&T> {
        let node = unsafe { utils::prev(self.node)? };
        unsafe { Some(node.as_ref()) }
    }
    pub fn peek_next_mut(&mut self) -> Option<Pin<&mut T>> {
        let mut node = utils::next(self.node)?;
        unsafe { Some(Pin::new_unchecked(node.as_mut())) }
    }
    pub fn peek_prev_mut(&mut self) -> Option<Pin<&mut T>> {
        let mut node = unsafe { utils::prev(self.node)? };
        unsafe { Some(Pin::new_unchecked(node.as_mut())) }
    }
}

pub struct VacantEntry<'a, T>
where
    T: Linked + ?Sized,
{
    pub(crate) parent_and_side: Option<(NonNull<T>, Side)>,
    pub(crate) _tree: &'a mut WAVLTree<T>,
}

impl<'a, T> VacantEntry<'a, T>
where
    T: Linked + ?Sized,
{
    pub fn peek_next(&self) -> Option<&T> {
        Some(unsafe { self.peek_next_inner()?.as_ref() })
    }
    pub fn peek_prev(&self) -> Option<&T> {
        Some(unsafe { self.peek_prev_inner()?.as_ref() })
    }
    pub fn peek_next_mut(&mut self) -> Option<Pin<&mut T>> {
        let mut node = self.peek_next_inner()?;
        unsafe { Some(Pin::new_unchecked(node.as_mut())) }
    }
    pub fn peek_prev_mut(&mut self) -> Option<Pin<&mut T>> {
        let mut node = self.peek_prev_inner()?;
        unsafe { Some(Pin::new_unchecked(node.as_mut())) }
    }
    pub fn insert(self, element: T::Handle) -> Pin<&'a mut T> {
        let mut ptr = T::into_ptr(element);
        debug_assert_ne!(self._tree.root, Some(ptr));

        let ptr_links = unsafe { T::links(ptr).as_mut() };
        assert!(!ptr_links.is_linked());

        let was_leaf = if let Some((parent, side)) = self.parent_and_side {
            let parent_links = unsafe { T::links(parent).as_mut() };

            let was_leaf = parent_links.is_leaf();
            ptr_links.replace_parent(Some(parent));
            parent_links.replace_child(side, Some(ptr));
            was_leaf
        } else {
            debug_assert!(self._tree.root.is_none());
            self._tree.root = Some(ptr);
            false
        };

        self._tree.size += 1;
        unsafe {
            T::after_insert(Pin::new_unchecked(ptr.as_mut()));
        }

        if was_leaf {
            self._tree.balance_after_insert(ptr);
        }

        unsafe { Pin::new_unchecked(ptr.as_mut()) }
    }

    fn peek_next_inner(&self) -> Link<T> {
        let (parent, side) = self.parent_and_side?;
        let parent_links = unsafe { T::links(parent).as_ref() };

        if let Some(right) = parent_links.right()
            && side == Side::Left
        {
            // If we have a right sibling, the next node is its left-most child
            Some(utils::find_minimum(right))
        } else {
            let mut parent = Some(parent);

            while let Some(_parent) = parent {
                let parent_links = unsafe { T::links(_parent).as_ref() };
                // if we have a parent, and we're not their right/greater child, that parent is our
                // next node
                if side == Side::Left {
                    return Some(_parent);
                }

                parent = parent_links.parent();
            }

            None
        }
    }

    fn peek_prev_inner(&self) -> Link<T> {
        let (parent, side) = self.parent_and_side?;
        let parent_links = unsafe { T::links(parent).as_ref() };

        if let Some(left) = parent_links.left()
            && side == Side::Right
        {
            // If we have a left sibling, the next node is its right-most child
            Some(utils::find_maximum(left))
        } else {
            let mut parent = Some(parent);

            while let Some(_parent) = parent {
                let parent_links = unsafe { T::links(_parent).as_ref() };
                // if we have a parent, and we're not their left/smaller child, that parent is our
                // previous node
                if side == Side::Right {
                    return Some(_parent);
                }

                parent = parent_links.parent();
            }

            None
        }
    }
}
