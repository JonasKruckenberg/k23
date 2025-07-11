// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::pin::Pin;

use crate::{Link, Linked, WAVLTree, utils};

/// A cursor which provides read-only access to a [`WAVLTree`].
#[must_use]
pub struct Cursor<'a, T>
where
    T: Linked + ?Sized,
{
    pub(crate) current: Link<T>,
    pub(crate) _tree: &'a WAVLTree<T>,
}

impl<'a, T> Cursor<'a, T>
where
    T: Linked + ?Sized,
{
    /// Returns the raw pointer to the current node
    ///
    /// # Safety
    ///
    /// Caller has to ensure the ptr is *never* used to move out of the current location, as the tree
    /// requires pinned memory locations.
    pub const unsafe fn get_ptr(&self) -> Link<T> {
        self.current
    }

    pub const fn get(&self) -> Option<&'a T> {
        if let Some(ptr) = self.current {
            Some(unsafe { ptr.as_ref() })
        } else {
            None
        }
    }

    pub fn move_next(&mut self) {
        if let Some(current) = self.current {
            self.current = utils::next(current);
        } else {
            self.current = None;
        }
    }
    pub fn move_prev(&mut self) {
        if let Some(current) = self.current {
            self.current = unsafe { utils::prev(current) };
        } else {
            self.current = None;
        }
    }
    pub fn peek_prev(&self) -> Option<&'a T> {
        if let Some(current) = self.current {
            let prev = unsafe { utils::prev(current)? };
            unsafe { Some(prev.as_ref()) }
        } else {
            None
        }
    }
    pub fn peek_next(&self) -> Option<&'a T> {
        if let Some(current) = self.current {
            let next = utils::next(current)?;
            unsafe { Some(next.as_ref()) }
        } else {
            None
        }
    }
}

/// A cursor which provides mutable access to a [`WAVLTree`].
#[must_use]
pub struct CursorMut<'a, T>
where
    T: Linked + ?Sized,
{
    pub(crate) current: Link<T>,
    pub(crate) _tree: &'a mut WAVLTree<T>,
}

impl<'a, T> CursorMut<'a, T>
where
    T: Linked + ?Sized,
{
    /// Returns the raw pointer to the current node
    ///
    /// # Safety
    ///
    /// Caller has to ensure the ptr is *never* used to move out of the current location, as the tree
    /// requires pinned memory locations.
    pub const unsafe fn get_ptr(&self) -> Link<T> {
        self.current
    }

    pub fn get(&self) -> Option<&'a T> {
        unsafe { self.current.map(|ptr| ptr.as_ref()) }
    }

    pub const fn get_mut(&mut self) -> Option<Pin<&'a mut T>> {
        if let Some(mut ptr) = self.current {
            Some(unsafe { Pin::new_unchecked(ptr.as_mut()) })
        } else {
            None
        }
    }
    pub fn move_next(&mut self) {
        if let Some(current) = self.current {
            self.current = utils::next(current);
        } else {
            self.current = None;
        }
    }
    pub fn move_prev(&mut self) {
        if let Some(current) = self.current {
            self.current = unsafe { utils::prev(current) };
        } else {
            self.current = None;
        }
    }
    pub fn remove(&mut self) -> Option<T::Handle> {
        let handle = self._tree.remove_internal(self.current?);
        self.current = None;
        Some(handle)
    }
    pub fn peek_prev(&self) -> Option<&'a T> {
        if let Some(current) = self.current {
            let prev = unsafe { utils::prev(current)? };
            unsafe { Some(prev.as_ref()) }
        } else {
            None
        }
    }
    pub fn peek_next(&self) -> Option<&'a T> {
        if let Some(current) = self.current {
            let next = utils::next(current)?;
            unsafe { Some(next.as_ref()) }
        } else {
            None
        }
    }
    pub fn peek_prev_mut(&self) -> Option<Pin<&'a mut T>> {
        if let Some(current) = self.current {
            let mut prev = unsafe { utils::prev(current)? };
            unsafe { Some(Pin::new_unchecked(prev.as_mut())) }
        } else {
            None
        }
    }
    pub fn peek_next_mut(&self) -> Option<Pin<&'a mut T>> {
        if let Some(current) = self.current {
            let mut next = utils::next(current)?;
            unsafe { Some(Pin::new_unchecked(next.as_mut())) }
        } else {
            None
        }
    }
    pub fn as_cursor(&self) -> Cursor<'_, T> {
        Cursor {
            current: self.current,
            _tree: self._tree,
        }
    }
}
