#![cfg_attr(not(test), no_std)]

use core::cell::UnsafeCell;
use core::marker::PhantomPinned;
use core::ptr::NonNull;
use core::{fmt, mem, ptr};
use core::iter::FusedIterator;
use core::pin::Pin;

pub unsafe trait Linked {
    /// The handle owning nodes in the tree.
    ///
    /// This type must have ownership over a `Self`-typed value. When a `Handle`
    /// is dropped, it should drop the corresponding `Linked` type.
    ///
    /// A quintessential example of a `Handle` is `Box`.
    type Handle;

    // Required methods
    /// Convert a [`Self::Handle`] to a raw pointer to `Self`, taking ownership
    /// of it in the process.
    fn into_ptr(r: Self::Handle) -> NonNull<Self>;
    /// Convert a raw pointer to Self into an owning Self::Handle.
    ///
    /// # Safety
    /// This function is safe to call when:
    ///
    /// It is valid to construct a Self::Handle from a`raw pointer
    /// The pointer points to a valid instance of Self (e.g. it does not dangle).
    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle;
    /// Return the links of the node pointed to by ptr.
    ///
    /// # Safety
    /// This function is safe to call when:
    ///
    /// It is valid to construct a Self::Handle from a`raw pointer
    /// The pointer points to a valid instance of Self (e.g. it does not dangle).
    /// See the [the trait-level documentation](#implementing-linkedlinks) documentation for details on how to correctly implement this method.
    unsafe fn links(ptr: NonNull<Self>) -> NonNull<Links<Self>>;
}

type Link<T> = Option<NonNull<T>>;

pub struct List<T>
where
    T: Linked + ?Sized,
{
    head: Link<T>,
    tail: Link<T>,
    len: usize,
}

unsafe impl<T: Linked + ?Sized> Send for List<T> where T: Send {}
unsafe impl<T: Linked + ?Sized> Sync for List<T> where T: Sync {}

impl<T> Drop for List<T>
where
    T: Linked + ?Sized,
{
    fn drop(&mut self) {
        while let Some(node) = self.pop_front() {
            drop(node);
        }

        debug_assert!(self.is_empty());
    }
}

impl<T> Default for List<T>
where
    T: Linked + ?Sized,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for List<T>
where
    T: Linked + ?Sized,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("List")
            .field("head", &self.head)
            .field("tail", &self.tail)
            .field("len", &self.len)
            .finish()
    }
}

impl<T> List<T>
where
    T: Linked + ?Sized,
{
    /// Creates a new, empty list.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            head: None,
            tail: None,
            len: 0,
        }
    }

    /// Returns the length of the list.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if this list is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        if self.head.is_none() {
            debug_assert!(
                self.tail.is_none(),
                "inconsistent state: head is None, but tail is not"
            );
            debug_assert_eq!(
                self.len, 0,
                "inconsistent state: a list was empty, but its length was not zero"
            );
            return true;
        }

        debug_assert_ne!(
            self.len, 0,
            "inconsistent state: a list was not empty, but its length was zero"
        );
        false
    }

    pub fn push_back(&mut self, element: T::Handle) {
        let ptr = T::into_ptr(element);
        assert_ne!(self.tail, Some(ptr));

        unsafe {
            T::links(ptr).as_mut().replace_next(None);
            T::links(ptr).as_mut().replace_prev(self.tail);
            if let Some(tail) = self.tail {
                T::links(tail).as_mut().replace_next(Some(ptr));
            }
        }

        self.tail = Some(ptr);
        if self.head.is_none() {
            self.head = Some(ptr);
        }

        self.len += 1;
    }

    pub fn push_front(&mut self, element: T::Handle) {
        let ptr = T::into_ptr(element);
        assert_ne!(self.head, Some(ptr));

        unsafe {
            T::links(ptr).as_mut().replace_next(self.head);
            T::links(ptr).as_mut().replace_prev(None);
            if let Some(head) = self.head {
                T::links(head).as_mut().replace_prev(Some(ptr));
            }
        }

        self.head = Some(ptr);

        if self.tail.is_none() {
            self.tail = Some(ptr);
        }

        self.len += 1;
    }

    pub fn pop_back(&mut self) -> Option<T::Handle> {
        let tail = self.tail?;
        self.len -= 1;

        unsafe {
            let mut tail_links = T::links(tail);
            self.tail = tail_links.as_ref().prev();

            if let Some(prev) = tail_links.as_mut().prev() {
                T::links(prev).as_mut().replace_next(None);
            } else {
                self.head = None;
            }

            tail_links.as_mut().unlink();
            Some(T::from_ptr(tail))
        }
    }

    pub fn pop_front(&mut self) -> Option<T::Handle> {
        let head = self.head?;
        self.len -= 1;

        unsafe {
            let mut head_links = T::links(head);
            self.head = head_links.as_ref().next();

            if let Some(next) = head_links.as_mut().next() {
                T::links(next).as_mut().replace_prev(None);
            } else {
                self.tail = None;
            }

            head_links.as_mut().unlink();
            Some(T::from_ptr(head))
        }
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            _list: self,
            curr: self.head,
            curr_back: self.tail,
            len: self.len,
        }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut {
            curr: self.head,
            curr_back: self.tail,
            len: self.len,
            _list: self,
        }
    }

    pub fn cursor_front(&self) -> Cursor<'_, T> {
        Cursor {
            current: self.head,
            _list: self,
        }
    }
    pub fn cusor_front_mut(&mut self) -> CursorMut<'_, T> {
        CursorMut {
            current: self.head,
            list: self,
        }
    }
    pub fn cursor_back(&self) -> Cursor<'_, T> {
        Cursor {
            current: self.tail,
            _list: self,
        }
    }
    pub fn cursor_back_mut(&mut self) -> CursorMut<'_, T> {
        CursorMut {
            current: self.tail,
            list: self,
        }
    }
    
    pub fn assert_valid(&self) {
        let Some(head) = self.head else {
            assert!(
                self.tail.is_none(),
                "if the linked list's head is null, the tail must also be null"
            );
            assert_eq!(
                self.len, 0,
                "if a linked list's head is null, its length must be 0"
            );
            return;
        };

        assert_ne!(
            self.len, 0,
            "if a linked list's head is not null, its length must be greater than 0"
        );

        assert_ne!(
            self.tail, None,
            "if the linked list has a head, it must also have a tail"
        );
        let tail = self.tail.unwrap();

        let head_links = unsafe { T::links(head) };
        let tail_links = unsafe { T::links(tail) };
        let head_links = unsafe { head_links.as_ref() };
        let tail_links = unsafe { tail_links.as_ref() };
        if ptr::eq(head.as_ptr(), tail.as_ptr()) {
            assert_eq!(
                head_links, tail_links,
                "if the head and tail nodes are the same, their links must be the same"
            );
            assert_eq!(
                head_links.next(),
                None,
                "if the linked list has only one node, it must not be linked"
            );
            assert_eq!(
                head_links.prev(),
                None,
                "if the linked list has only one node, it must not be linked"
            );
            return;
        }

        let mut curr = Some(head);
        let mut actual_len = 0;
        while let Some(node) = curr {
            let links = unsafe { T::links(node) };
            let links = unsafe { links.as_ref() };
            links.assert_valid(head_links, tail_links);
            curr = links.next();
            actual_len += 1;
        }

        assert_eq!(
            self.len, actual_len,
            "linked list's actual length did not match its `len` variable"
        );
    }
}

pub struct Links<T>
where
    T: Linked + ?Sized,
{
    inner: UnsafeCell<LinksInner<T>>,
}

#[repr(C)]
struct LinksInner<T>
where
    T: Linked + ?Sized,
{
    next: Link<T>,
    prev: Link<T>,
    /// Linked list links must always be `!Unpin`, in order to ensure that they
    /// never recieve LLVM `noalias` annotations; see also
    /// <https://github.com/rust-lang/rust/issues/63818>.
    _unpin: PhantomPinned,
}

impl<T> Default for Links<T>
where
    T: Linked + ?Sized,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for Links<T>
where
    T: Linked + ?Sized,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Links")
            .field("self", &format_args!("{self:p}"))
            .field("prev", &self.prev())
            .field("next", &self.next())
            .finish()
    }
}

impl<T> PartialEq for Links<T>
where
    T: Linked + ?Sized,
{
    fn eq(&self, other: &Self) -> bool {
        self.next() == other.next() && self.prev() == other.prev()
    }
}

/// # Safety
///
/// Types containing [`Links`] may be `Send`: the pointers within the `Links` may
/// mutably alias another value, but the links can only be _accessed_ by the
/// owner of the [`List`] itself, because the pointers are private. As long as
/// [`List`] upholds its own invariants, `Links` should not make a type `!Send`.
unsafe impl<T> Send for Links<T> where T: Send + Linked + ?Sized {}

/// # Safety
///
/// Types containing [`Links`] may be `Sync`: the pointers within the `Links` may
/// mutably alias another value, but the links can only be _accessed_ by the
/// owner of the [`List`] itself, because the pointers are private. As long as
/// [`List`] upholds its own invariants, `Links` should not make a type `!Sync`.
unsafe impl<T> Sync for Links<T> where T: Sync + Linked + ?Sized {}

impl<T> Links<T>
where
    T: Linked + ?Sized,
{
    /// Returns new links for a [doubly-linked intrusive list](List).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(LinksInner {
                next: None,
                prev: None,
                _unpin: PhantomPinned,
            }),
        }
    }

    /// Returns `true` if this node is currently linked to a [`List`].
    pub fn is_linked(&self) -> bool {
        self.next().is_some() || self.prev().is_some()
    }

    fn unlink(&mut self) {
        self.inner.get_mut().next = None;
        self.inner.get_mut().prev = None;
    }

    #[inline]
    fn next(&self) -> Link<T> {
        unsafe { (*self.inner.get()).next }
    }

    #[inline]
    fn prev(&self) -> Link<T> {
        unsafe { (*self.inner.get()).prev }
    }

    #[inline]
    fn replace_next(&mut self, next: Link<T>) -> Link<T> {
        mem::replace(&mut self.inner.get_mut().next, next)
    }

    #[inline]
    fn replace_prev(&mut self, prev: Link<T>) -> Link<T> {
        mem::replace(&mut self.inner.get_mut().prev, prev)
    }

    fn assert_valid(&self, head: &Self, tail: &Self) {
        if ptr::eq(self, head) {
            assert_eq!(
                self.prev(),
                None,
                "head node must not have a prev link; node={self:#?}",
            );
        }

        if ptr::eq(self, tail) {
            assert_eq!(
                self.next(),
                None,
                "tail node must not have a next link; node={self:#?}",
            );
        }

        assert_ne!(
            self.next(),
            self.prev(),
            "node cannot be linked in a loop; node={self:#?}",
        );

        if let Some(next) = self.next() {
            assert_ne!(
                unsafe { T::links(next) },
                NonNull::from(self),
                "node's next link cannot be to itself; node={self:#?}",
            );
        }
        if let Some(prev) = self.prev() {
            assert_ne!(
                unsafe { T::links(prev) },
                NonNull::from(self),
                "node's prev link cannot be to itself; node={self:#?}",
            );
        }
    }
}

#[derive(Clone)]
pub struct Cursor<'a, T> where T: Linked + ?Sized {
    current: Link<T>,
    _list: &'a List<T>,
}
impl<'a, T> Cursor<'a, T> where T: Linked + ?Sized {
    pub fn get(&self) -> Option<&'a T> { self.current.map(|ptr| unsafe { ptr.as_ref() }) }
    pub fn get_ptr(&self) -> Link<T> { self.current }
    pub fn move_next(&mut self) {
        if let Some(ptr) = self.current {
            self.current = unsafe { next(ptr) };
        }
    }
    pub fn move_prev(&mut self) {
        if let Some(ptr) = self.current {
            self.current = unsafe { prev(ptr) };
        }
    }
    pub fn peek_prev(&self) -> Option<&T> {
        if let Some(ptr) = self.current {
            let prev = unsafe { prev(ptr) };
            prev.map(|ptr| unsafe { ptr.as_ref() })
        } else {
            None
        }
    }
    pub fn peek_next(&self) -> Option<&T> {
        if let Some(ptr) = self.current {
            let next = unsafe { next(ptr) };
            next.map(|ptr| unsafe { ptr.as_ref() })
        } else {
            None
        }
    }
}

pub struct CursorMut<'a, T> where T: Linked + ?Sized {
    current: Link<T>,
    list: &'a mut List<T>,
}
impl<'a, T> CursorMut<'a, T> where T: Linked + ?Sized {
    pub fn get(&self) -> Option<&'a T> { self.current.map(|ptr| unsafe { ptr.as_ref() }) }
    pub fn get_ptr(&self) -> Link<T> { self.current }
    pub fn get_mut(&mut self) -> Option<Pin<&'a mut T>> {
        if let Some(mut ptr) = self.current {
            Some(unsafe { Pin::new_unchecked(ptr.as_mut()) })
        } else {
            None
        }
    }
    pub fn move_next(&mut self) {
        if let Some(ptr) = self.current {
            self.current = unsafe { next(ptr) };
        }
    }
    pub fn move_prev(&mut self) {
        if let Some(ptr) = self.current {
            self.current = unsafe { prev(ptr) };
        }
    }
    pub fn peek_prev(&self) -> Option<&T> {
        if let Some(ptr) = self.current {
            let prev = unsafe { prev(ptr) };
            prev.map(|ptr| unsafe { ptr.as_ref() })
        } else {
            None
        }
    }
    pub fn peek_next(&self) -> Option<&T> {
        if let Some(ptr) = self.current {
            let next = unsafe { next(ptr) };
            next.map(|ptr| unsafe { ptr.as_ref() })
        } else {
            None
        }
    }
    pub fn as_cursor(&self) -> Cursor<'_, T> {
        Cursor {
            current: self.current,
            _list: self.list,
        }
    }
    pub fn remove(&mut self) -> Option<T::Handle> {
        unsafe {
            let node = self.current?;
            let node_links = T::links(node).as_mut();

            let prev = node_links.replace_prev(None);
            let next = node_links.replace_next(None);

            if let Some(prev) = prev {
                T::links(prev).as_mut().replace_next(next);
            } else {
                debug_assert_ne!(Some(node), next, "node must not be linked to itself");
                self.list.head = next;
            }

            if let Some(next) = next {
                T::links(next).as_mut().replace_prev(prev);
            } else {
                debug_assert_ne!(Some(node), next, "node must not be linked to itself");
                self.list.tail = prev;
            }

            self.list.len -= 1;
            Some(T::from_ptr(self.current?))
        }
    }
}

pub struct Iter<'a, T> where T: Linked + ?Sized {
    _list: &'a List<T>,

    /// The current node when iterating head -> tail.
    curr: Link<T>,

    /// The current node when iterating tail -> head.
    ///
    /// This is used by the [`DoubleEndedIterator`] impl.
    curr_back: Link<T>,

    /// The number of remaining entries in the iterator.
    len: usize,
}

impl<'a, T> Iterator for Iter<'a, T> where T: Linked + ?Sized {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.len == 0 {
            return None;
        }

        let curr = self.curr.take()?;
        self.len -= 1;
        unsafe {
            // safety: it is safe for us to borrow `curr`, because the iterator
            // borrows the `List`, ensuring that the list will not be dropped
            // while the iterator exists. the returned item will not outlive the
            // iterator.
            self.curr = T::links(curr).as_ref().next();
            Some(curr.as_ref())
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }
}

impl<T> ExactSizeIterator for Iter<'_, T> where T: Linked + ?Sized {
    fn len(&self) -> usize {
        self.len
    }
}

impl<'a, T> DoubleEndedIterator for Iter<'a, T> where T: Linked + ?Sized {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.len == 0 {
            return None;
        }

        let curr_back = self.curr_back.take()?;
        self.len -= 1;
        unsafe {
            // safety: it is safe for us to borrow `curr`, because the iterator
            // borrows the `List`, ensuring that the list will not be dropped
            // while the iterator exists. the returned item will not outlive the
            // iterator.
            self.curr_back = T::links(curr_back).as_ref().prev();
            Some(curr_back.as_ref())
        }
    }
}

impl<T> FusedIterator for Iter<'_, T> where T: Linked + ?Sized {}

pub struct IterMut<'a, T> where T: Linked + ?Sized {
    _list: &'a mut List<T>,

    /// The current node when iterating head -> tail.
    curr: Link<T>,

    /// The current node when iterating tail -> head.
    ///
    /// This is used by the [`DoubleEndedIterator`] impl.
    curr_back: Link<T>,

    /// The number of remaining entries in the iterator.
    len: usize,
}

impl<'a, T> Iterator for IterMut<'a, T> where T: Linked + ?Sized {
    type Item = Pin<&'a mut T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.len == 0 {
            return None;
        }

        let mut curr = self.curr.take()?;
        self.len -= 1;
        unsafe {
            // safety: it is safe for us to borrow `curr`, because the iterator
            // borrows the `List`, ensuring that the list will not be dropped
            // while the iterator exists. the returned item will not outlive the
            // iterator.
            self.curr = T::links(curr).as_ref().next();

            // safety: pinning the returned element is actually *necessary* to
            // uphold safety invariants here. if we returned `&mut T`, the
            // element could be `mem::replace`d out of the list, invalidating
            // any pointers to it. thus, we *must* pin it before returning it.
            Some(Pin::new_unchecked(curr.as_mut()))
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }
}

impl<T> ExactSizeIterator for IterMut<'_, T> where T: Linked + ?Sized {
    fn len(&self) -> usize {
        self.len
    }
}

impl<'a, T> DoubleEndedIterator for IterMut<'a, T> where T: Linked + ?Sized {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.len == 0 {
            return None;
        }

        let mut curr_back = self.curr_back.take()?;
        self.len -= 1;
        unsafe {
            // safety: it is safe for us to borrow `curr`, because the iterator
            // borrows the `List`, ensuring that the list will not be dropped
            // while the iterator exists. the returned item will not outlive the
            // iterator.
            self.curr_back = T::links(curr_back).as_ref().prev();

            // safety: pinning the returned element is actually *necessary* to
            // uphold safety invariants here. if we returned `&mut T`, the
            // element could be `mem::replace`d out of the list, invalidating
            // any pointers to it. thus, we *must* pin it before returning it.
            Some(Pin::new_unchecked(curr_back.as_mut()))
        }
    }
}

impl<T> FusedIterator for IterMut<'_, T> where T: Linked + ?Sized {}

unsafe fn next<T>(node: NonNull<T>) -> Link<T>
where
    T: Linked + ?Sized,
{
    unsafe { T::links(node).as_ref().next() }
}
unsafe fn prev<T>(node: NonNull<T>) -> Link<T>
where
    T: Linked + ?Sized,
{
    unsafe { T::links(node).as_ref().prev() }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    use core::mem::offset_of;
    use super::*;

    #[derive(Debug)]
    struct TestNode {
        links: Links<TestNode>,
        value: usize,
    }

    impl TestNode {
        fn new(value: usize) -> Pin<Box<Self>> {
            Box::pin(
            Self {
                links: Links::new(),
                value,
            })
        }
    }

    unsafe impl Linked for TestNode {
        type Handle = Pin<Box<Self>>;

        /// Convert an owned `Handle` into a raw pointer
        fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
            unsafe { NonNull::from(Box::leak(Pin::into_inner_unchecked(handle))) }
        }

        /// Convert a raw pointer back into an owned `Handle`.
        unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
            // Safety: `NonNull` *must* be constructed from a pinned reference
            // which the tree implementation upholds.
            Pin::new_unchecked(Box::from_raw(ptr.as_ptr()))
        }

        unsafe fn links(ptr: NonNull<Self>) -> NonNull<Links<Self>> {
            ptr.map_addr(|addr| {
                let offset = offset_of!(Self, links);
                addr.checked_add(offset).unwrap()
            })
                .cast()
        }
    }

    #[test]
    fn push_back() {
        let mut list: List<TestNode> = List::new();

        list.push_back(TestNode::new(0));
        list.push_back(TestNode::new(1));
        list.push_back(TestNode::new(2));
        list.push_back(TestNode::new(3));

        let v = list.iter().map(|n| n.value).collect::<Vec<_>>();
        assert_eq!(v, [0,1,2,3])
    }

    #[test]
    fn push_front() {
        let mut list: List<TestNode> = List::new();

        list.push_front(TestNode::new(0));
        list.push_front(TestNode::new(1));
        list.push_front(TestNode::new(2));
        list.push_front(TestNode::new(3));

        let v = list.iter().map(|n| n.value).collect::<Vec<_>>();
        assert_eq!(v, [3, 2, 1, 0,])
    }

    #[test]
    fn pop_front() {
        let mut list: List<TestNode> = List::new();

        list.push_back(TestNode::new(0));
        list.push_back(TestNode::new(1));
        list.push_back(TestNode::new(2));
        list.push_back(TestNode::new(3));

        assert_eq!(list.pop_front().unwrap().value, 0);
        assert_eq!(list.pop_front().unwrap().value, 1);
        assert_eq!(list.pop_front().unwrap().value, 2);
        assert_eq!(list.pop_front().unwrap().value, 3);
    }
}