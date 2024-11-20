// pub fn lower_bound(&self) -> Cursor<'_, T> {
//     todo!()
// }
// pub fn lower_bound_mut(&mut self) -> CursorMut<'_, T> {
//     todo!()
// }
// pub fn upper_bound(&self) -> Cursor<'_, T> {
//     todo!()
// }
// pub fn upper_bound_mut(&mut self) -> CursorMut<'_, T> {
//     todo!()
// }
use crate::WAVLTree;
use crate::{Link, Linked};
use core::iter::FusedIterator;
use core::pin::Pin;
use core::ptr::NonNull;

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
            self.current = unsafe { next(current) };
        } else {
            self.current = None
        }
    }
    pub fn move_prev(&mut self) {
        if let Some(current) = self.current {
            self.current = unsafe { prev(current) };
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
            self.current = unsafe { next(current) };
        } else {
            self.current = None
        }
    }
    pub fn move_prev(&mut self) {
        if let Some(current) = self.current {
            self.current = unsafe { prev(current) };
        } else {
            self.current = None
        }
    }
    pub fn remove_current(&mut self) -> T::Handle {
        todo!()
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

/// An iterator over references to the entries of a [`WAVLTree`].
pub struct Iter<'a, T: Linked + ?Sized> {
    pub(crate) head: Link<T>,
    pub(crate) tail: Link<T>,
    pub(crate) _tree: &'a WAVLTree<T>,
}
impl<'a, T> Clone for Iter<'a, T>
where
    T: Linked + ?Sized,
{
    #[inline]
    fn clone(&self) -> Iter<'a, T> {
        Iter {
            head: self.head,
            tail: self.tail,
            _tree: self._tree,
        }
    }
}
impl<'a, T> Iterator for Iter<'a, T>
where
    T: Linked + ?Sized + 'a,
{
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        let head = self.head?;

        if Some(head) == self.tail {
            self.head = None;
            self.tail = None;
        } else {
            self.head = unsafe { next(head) };
        }

        Some(unsafe { head.as_ref() })
    }
}
impl<'a, T> DoubleEndedIterator for Iter<'a, T>
where
    T: Linked + ?Sized + 'a,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        let tail = self.tail?;

        if Some(tail) == self.head {
            self.head = None;
            self.tail = None;
        } else {
            self.tail = unsafe { prev(tail) };
        }

        Some(unsafe { tail.as_ref() })
    }
}
impl<'a, T> FusedIterator for Iter<'a, T> where T: Linked + ?Sized + 'a {}

/// An iterator over mutable references to the entries of a [`WAVLTree`].
pub struct IterMut<'a, T: Linked + ?Sized> {
    pub(crate) head: Link<T>,
    pub(crate) tail: Link<T>,
    pub(crate) _tree: &'a mut WAVLTree<T>,
}
impl<'a, T> Iterator for IterMut<'a, T>
where
    T: Linked + ?Sized + 'a,
{
    type Item = Pin<&'a mut T>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut head = self.head?;

        if Some(head) == self.tail {
            self.head = None;
            self.tail = None;
        } else {
            self.head = unsafe { next(head) };
        }

        Some(unsafe { Pin::new_unchecked(head.as_mut()) })
    }
}
impl<'a, T> DoubleEndedIterator for IterMut<'a, T>
where
    T: Linked + ?Sized + 'a,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        let mut tail = self.tail?;

        if Some(tail) == self.head {
            self.head = None;
            self.tail = None;
        } else {
            self.tail = unsafe { prev(tail) };
        }

        Some(unsafe { Pin::new_unchecked(tail.as_mut()) })
    }
}
impl<'a, T> FusedIterator for IterMut<'a, T> where T: Linked + ?Sized + 'a {}

unsafe fn next<T>(node: NonNull<T>) -> Link<T>
where
    T: Linked + ?Sized,
{
    let node_links = T::links(node).as_ref();

    // If we have a right child, its least descendant is our previous node
    if let Some(right) = node_links.right() {
        Some(crate::utils::find_minimum(right))
    } else {
        let mut curr = node;

        loop {
            if let Some(parent) = T::links(curr).as_ref().parent() {
                let parent_links = T::links(parent).as_ref();

                // if we have a parent, and we're not their right/greater child, that parent is our
                // previous node
                if parent_links.right() != Some(curr) {
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

unsafe fn prev<T>(node: NonNull<T>) -> Link<T>
where
    T: Linked + ?Sized,
{
    let node_links = T::links(node).as_ref();

    // If we have a left child, its greatest descendant is our previous node
    if let Some(left) = node_links.left() {
        Some(crate::utils::find_maximum(left))
    } else {
        let mut curr = node;

        loop {
            if let Some(parent) = T::links(curr).as_ref().parent() {
                let parent_links = T::links(parent).as_ref();

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
