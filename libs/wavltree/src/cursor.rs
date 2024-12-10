use core::pin::Pin;
use crate::{utils, Link, Linked, WAVLTree};

/// A cursor which provides read-only access to a [`WAVLTree`].
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
    pub unsafe fn get_ptr(&self) -> Link<T> {
        self.current
    }
    pub fn get(&self) -> Option<&'a T> {
        unsafe { self.current.map(|ptr| ptr.as_ref()) }
    }
    pub fn move_next(&mut self) {
        if let Some(current) = self.current {
            self.current = unsafe { utils::next(current) };
        } else {
            self.current = None
        }
    }
    pub fn move_prev(&mut self) {
        if let Some(current) = self.current {
            self.current = unsafe { utils::prev(current) };
        } else {
            self.current = None
        }
    }
    pub fn peek_prev(&self) -> Option<&'a T> {
        todo!()
    }
    pub fn peek_next(&self) -> Option<&'a T> {
        todo!()
    }
}

/// A cursor which provides mutable access to a [`WAVLTree`].
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
    pub unsafe fn get_ptr(&self) -> Link<T> {
        self.current
    }
    pub fn get(&self) -> Option<&'a T> {
        unsafe { self.current.map(|ptr| ptr.as_ref()) }
    }
    pub fn get_mut(&mut self) -> Option<Pin<&'a mut T>> {
        unsafe { self.current.map(|mut ptr| Pin::new_unchecked(ptr.as_mut())) }
    }
    pub fn move_next(&mut self) {
        if let Some(current) = self.current {
            self.current = unsafe { utils::next(current) };
        } else {
            self.current = None
        }
    }
    pub fn move_prev(&mut self) {
        if let Some(current) = self.current {
            self.current = unsafe { utils::prev(current) };
        } else {
            self.current = None
        }
    }
    pub fn remove(&mut self) -> Option<T::Handle> {
        unsafe {
            let handle = self._tree.remove_internal(self.current?);
            Some(handle)
        }
    }
    pub fn peek_prev(&self) -> Option<Pin<&T>> {
        todo!()
    }
    pub fn peek_next(&self) -> Option<Pin<&T>> {
        todo!()
    }
    pub fn peek_prev_mut(&self) -> Option<Pin<&mut T>> {
        todo!()
    }
    pub fn peek_next_mut(&self) -> Option<Pin<&mut T>> {
        todo!()
    }
    pub fn as_cursor(&self) -> Cursor<'_, T> {
        Cursor {
            current: self.current,
            _tree: self._tree,
        }
    }
}