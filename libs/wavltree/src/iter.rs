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
use crate::{utils, WAVLTree};
use crate::{Link, Linked};
use core::iter::FusedIterator;
use core::pin::Pin;

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
            self.head = unsafe { utils::next(head) };
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
            self.head = unsafe { utils::next(head) };
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
