// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use core::iter::FusedIterator;
use core::pin::Pin;

use crate::{Link, Linked, WAVLTree, utils};

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
            self.head = utils::next(head);
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
            self.tail = unsafe { utils::prev(tail) };
        }

        Some(unsafe { tail.as_ref() })
    }
}
impl<'a, T> FusedIterator for Iter<'a, T> where T: Linked + ?Sized + 'a {}

/// An iterator over mutable references to the entries of a [`WAVLTree`].
pub struct IterMut<'a, T>
where
    T: Linked + ?Sized,
{
    pub(crate) head: Link<T>,
    pub(crate) tail: Link<T>,
    pub(crate) _tree: &'a mut WAVLTree<T>,
}
impl<T> IterMut<'_, T>
where
    T: Linked + ?Sized,
{
    pub fn tree(&mut self) -> &mut WAVLTree<T> {
        self._tree
    }
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
            self.head = utils::next(head);
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
            self.tail = unsafe { utils::prev(tail) };
        }

        Some(unsafe { Pin::new_unchecked(tail.as_mut()) })
    }
}
impl<'a, T> FusedIterator for IterMut<'a, T> where T: Linked + ?Sized + 'a {}

/// An iterator which consumes a [`WAVLTree`].
pub struct IntoIter<T>
where
    T: Linked + ?Sized,
{
    pub(crate) head: Link<T>,
    pub(crate) tail: Link<T>,
    pub(crate) _tree: WAVLTree<T>,
}
impl<T> Iterator for IntoIter<T>
where
    T: Linked + ?Sized,
{
    type Item = T::Handle;

    fn next(&mut self) -> Option<Self::Item> {
        let head = self.head?;
        let head_links = unsafe { T::links(head).as_mut() };
        let parent = head_links.parent();

        if let Some(parent) = parent {
            unsafe {
                T::links(parent).as_mut().replace_left(head_links.right());
            }
        } else {
            self._tree.root = head_links.right();
            if head_links.right().is_none() {
                self.tail = None;
            }
        }

        if let Some(right) = head_links.right() {
            unsafe {
                T::links(right).as_mut().replace_parent(parent);
            }
            self.head = Some(utils::find_minimum(right));
        } else {
            self.head = parent;
        }

        unsafe {
            // unlink the node from the tree and return
            head_links.unlink();
            Some(T::from_ptr(head))
        }
    }
}
impl<T> DoubleEndedIterator for IntoIter<T>
where
    T: Linked + ?Sized,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        let tail = self.tail?;
        let tail_links = unsafe { T::links(tail).as_mut() };
        let parent = tail_links.parent();

        if let Some(parent) = parent {
            unsafe {
                T::links(parent).as_mut().replace_right(tail_links.left());
            }
        } else {
            self._tree.root = tail_links.left();
            if tail_links.left().is_none() {
                self.tail = None;
            }
        }

        if let Some(left) = tail_links.left() {
            unsafe {
                T::links(left).as_mut().replace_parent(parent);
            }
            self.tail = Some(utils::find_maximum(left));
        } else {
            self.tail = parent;
        }

        unsafe {
            // unlink the node from the tree and return
            tail_links.unlink();
            Some(T::from_ptr(tail))
        }
    }
}
impl<T> FusedIterator for IntoIter<T> where T: Linked + ?Sized {}
