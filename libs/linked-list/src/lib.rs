// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![cfg_attr(not(test), no_std)]

use core::cell::UnsafeCell;
use core::iter::FusedIterator;
use core::marker::PhantomPinned;
use core::pin::Pin;
use core::ptr::NonNull;
use core::{fmt, mem, ptr};

/// Trait implemented by types which can be members of an intrusive doubly-linked list.
///
/// In order to be part of an intrusive WAVL tree, a type must contain a
/// `Links` type that stores the pointers to other nodes in the tree.
///
/// # Safety
///
/// This is unsafe to implement because it's the implementation's responsibility
/// to ensure that types implementing this trait are valid intrusive collection
/// nodes. In particular:
///
/// - Implementations **must** ensure that implementors are pinned in memory while they
///   are in an intrusive collection. While a given `Linked` type is in an intrusive
///   data structure, it may not be deallocated or moved to a different memory
///   location.
/// - The type implementing this trait **must not** implement [`Unpin`].
/// - Additional safety requirements for individual methods on this trait are
///   documented on those methods.
///
/// Failure to uphold these invariants will result in corruption of the
/// intrusive data structure, including dangling pointers.
///
/// # Implementing `Linked::links`
///
/// The [`Linked::links`] method provides access to a `Linked` type's `Links`
/// field through a [`NonNull`] pointer. This is necessary for a type to
/// participate in an intrusive structure, as it tells the intrusive structure
/// how to access the links to other parts of that data structure. However, this
/// method is somewhat difficult to implement correctly.
///
/// Suppose we have an element type like this:
/// ```rust
/// struct Entry {
///     links: linked_list::Links<Self>,
///     data: usize,
/// }
/// ```
///
/// The naive implementation of [`links`](Linked::links) for this `Entry` type
/// might look like this:
///
/// ```
/// use linked_list::Linked;
/// use core::ptr::NonNull;
///
/// # struct Entry {
/// #    links: linked_list::Links<Self>,
/// #    data: usize
/// # }
///
/// unsafe impl Linked for Entry {
///     # type Handle = NonNull<Self>;
///     # fn into_ptr(r: Self::Handle) -> NonNull<Self> { r }
///     # unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle { ptr }
///     // ...
///
///     unsafe fn links(mut target: NonNull<Self>) -> NonNull<linked_list::Links<Self>> {
///         // Borrow the target's `links` field.
///         let links = &mut target.as_mut().links;
///         // Convert that reference into a pointer.
///         NonNull::from(links)
///     }
/// }
/// ```
///
/// However, this implementation **is not sound** under [Stacked Borrows]! It
/// creates a temporary reference from the original raw pointer, and then
/// creates a new raw pointer from that temporary reference. Stacked Borrows
/// will reject this reborrow as unsound.[^1]
///
/// There are two ways we can implement [`Linked::links`] without creating a
/// temporary reference in this manner. The recommended one is to use the
/// [`ptr::addr_of_mut!`] macro, as follows:
///
/// ```
/// use core::ptr::{self, NonNull};
/// # use linked_list::Linked;
/// # struct Entry {
/// #    links: linked_list::Links<Self>,
/// #    data: usize,
/// # }
///
/// unsafe impl Linked for Entry {
///     # type Handle = NonNull<Self>;
///     # fn into_ptr(r: Self::Handle) -> NonNull<Self> { r }
///     # unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle { ptr }
///     // ...
///
///     unsafe fn links(target: NonNull<Self>) -> NonNull<linked_list::Links<Self>> {
///        // Note that we use the `map_addr` method here that is part of the strict-provenance
///         target
///             .map_addr(|addr| {
///                 // Using the `offset_of!` macro here to calculate the offset of the `links` field
///                 // in our overall struct.
///                 let offset = core::mem::offset_of!(Self, links);
///                 addr.checked_add(offset).unwrap()
///             })
///             .cast()
///     }
/// }
/// ```
///
/// [^1]: Note that code like this is not *currently* known to result in
///     miscompiles, but it is rejected by tools like Miri as being unsound.
///     Like all undefined behavior, there is no guarantee that future Rust
///     compilers will not miscompile code like this, with disastrous results.
///
/// [^2]: And two different fields cannot both be the first field at the same
///      time...by definition.
///
/// [intrusive collection]: crate#intrusive-data-structures
/// [`Unpin`]: Unpin
/// [doubly-linked list]: crate::list
/// [MSPC queue]: crate::mpsc_queue
/// [Stacked Borrows]: https://github.com/rust-lang/unsafe-code-guidelines/blob/master/wip/stacked-borrows.md
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
                "inconsistent state: head is None, but tail is not; self={self:?}",
            );
            debug_assert_eq!(
                self.len, 0,
                "inconsistent state: a list was empty, but its length was not zero; self={self:?}"
            );
            return true;
        }

        debug_assert_ne!(
            self.len, 0,
            "inconsistent state: a list was not empty, but its length was zero; self={self:?}"
        );
        false
    }

    pub fn push_back(&mut self, element: T::Handle) {
        let ptr = T::into_ptr(element);
        assert_ne!(self.tail, Some(ptr));

        unsafe {
            debug_assert!(
                !T::links(ptr).as_ref().is_linked(),
                "cannot insert an already linked node into a list"
            );

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
            debug_assert!(
                !T::links(ptr).as_ref().is_linked(),
                "cannot insert an already linked node into a list"
            );

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

    pub fn front(&self) -> Option<&T> {
        let node = self.head?;
        Some(unsafe { node.as_ref() })
    }

    pub fn front_mut(&mut self) -> Option<Pin<&mut T>> {
        let mut node = self.head?;
        let pin = unsafe {
            // Pin the reference to ensure intrusively linked
            // elements cannot be moved while in a collection.
            Pin::new_unchecked(node.as_mut())
        };
        Some(pin)
    }

    pub fn back(&self) -> Option<&T> {
        let node = self.tail?;
        Some(unsafe { node.as_ref() })
    }

    pub fn back_mut(&mut self) -> Option<Pin<&mut T>> {
        let mut node = self.tail?;
        let pin = unsafe {
            // Pin the reference to ensure intrusively linked
            // elements cannot be moved while in a collection.
            Pin::new_unchecked(node.as_mut())
        };
        Some(pin)
    }

    pub fn cursor_front(&self) -> Cursor<'_, T> {
        Cursor {
            current: self.head,
            _list: self,
        }
    }

    pub fn cursor_front_mut(&mut self) -> CursorMut<'_, T> {
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

    pub fn split_off(&mut self, at: usize) -> Option<Self> {
        let len = self.len();
        // what is the index of the last node that should be left in this list?
        let split_idx = match at {
            // trying to split at the 0th index. we can just return the whole
            // list, leaving `self` empty.
            0 => return Some(mem::replace(self, Self::new())),
            // trying to split at the last index. the new list will be empty.
            at if at == len => return Some(Self::new()),
            // we cannot split at an index that is greater than the length of
            // this list.
            at if at > len => return None,
            // otherwise, the last node in this list will be `at - 1`.
            at => at - 1,
        };

        let mut iter = self.iter();

        // advance to the node at `split_idx`, starting either from the head or
        // tail of the list.
        let dist_from_tail = len - 1 - split_idx;
        let split_node = if split_idx <= dist_from_tail {
            // advance from the head of the list.
            for _ in 0..split_idx {
                iter.next();
            }
            iter.curr
        } else {
            // advance from the tail of the list.
            for _ in 0..dist_from_tail {
                iter.next_back();
            }
            iter.curr_back
        };

        let Some(split_node) = split_node else {
            return Some(mem::replace(self, Self::new()));
        };

        // the head of the new list is the split node's `next` node (which is
        // replaced with `None`)
        let head = unsafe { T::links(split_node).as_mut().replace_next(None) };
        let tail = if let Some(head) = head {
            // since `head` is now the head of its own list, it has no `prev`
            // link any more.
            let _prev = unsafe { T::links(head).as_mut().replace_prev(None) };
            debug_assert_eq!(_prev, Some(split_node));

            // the tail of the new list is this list's old tail, if the split list
            // is not empty.
            self.tail.replace(split_node)
        } else {
            None
        };

        let split = Self {
            head,
            tail,
            len: self.len - at,
        };

        // update this list's length (note that this occurs after constructing
        // the new list, because we use this list's length to determine the new
        // list's length).
        self.len = at;

        Some(split)
    }

    pub fn append(&mut self, other: &mut Self) {
        let Some(tail) = self.tail else {
            // if this list is empty, simply replace it with `other`
            debug_assert!(self.is_empty());
            mem::swap(self, other);
            return;
        };

        if let Some((other_head, other_tail, other_len)) = other.take_all() {
            // attach the other list's head node to this list's tail node.
            unsafe {
                T::links(tail).as_mut().replace_next(Some(other_head));
                T::links(other_head).as_mut().replace_prev(Some(tail));
            }

            // this list's tail node is now the other list's tail node.
            self.tail = Some(other_tail);
            // this list's length increases by the other list's length, which
            // becomes 0.
            self.len += other_len;
        }
    }

    #[inline]
    fn take_all(&mut self) -> Option<(NonNull<T>, NonNull<T>, usize)> {
        let head = self.head.take()?;
        let tail = self.tail.take();
        debug_assert!(
            tail.is_some(),
            "if a list's `head` is `Some`, its tail must also be `Some`"
        );
        let tail = tail?;
        let len = mem::replace(&mut self.len, 0);
        debug_assert_ne!(
            len, 0,
            "if a list is non-empty, its `len` must be greater than 0"
        );
        Some((head, tail, len))
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

impl<T> Extend<T::Handle> for List<T>
where
    T: Linked + ?Sized,
{
    fn extend<I: IntoIterator<Item = T::Handle>>(&mut self, iter: I) {
        for item in iter {
            self.push_back(item);
        }
    }
}

impl<T> FromIterator<T::Handle> for List<T>
where
    T: Linked + ?Sized,
{
    fn from_iter<I: IntoIterator<Item = T::Handle>>(iter: I) -> Self {
        let mut out = Self::new();
        for item in iter.into_iter() {
            out.push_back(item);
        }
        out
    }
}

impl<'a, T: Linked + ?Sized> IntoIterator for &'a List<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T: Linked + ?Sized> IntoIterator for &'a mut List<T> {
    type Item = Pin<&'a mut T>;
    type IntoIter = IterMut<'a, T>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T: Linked + ?Sized> IntoIterator for List<T> {
    type Item = T::Handle;
    type IntoIter = IntoIter<T>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        IntoIter { list: self }
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
    /// never receive LLVM `noalias` annotations; see also
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

pub struct Cursor<'a, T>
where
    T: Linked + ?Sized,
{
    current: Link<T>,
    _list: &'a List<T>,
}

impl<T> Clone for Cursor<'_, T>
where
    T: Linked + ?Sized,
{
    fn clone(&self) -> Self {
        Self {
            current: self.current,
            _list: self._list,
        }
    }
}

impl<'a, T> Cursor<'a, T>
where
    T: Linked + ?Sized,
{
    pub fn get(&self) -> Option<&'a T> {
        self.current.map(|ptr| unsafe { ptr.as_ref() })
    }
    pub fn get_ptr(&self) -> Link<T> {
        self.current
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
}

pub struct CursorMut<'a, T>
where
    T: Linked + ?Sized,
{
    current: Link<T>,
    list: &'a mut List<T>,
}
impl<'a, T> CursorMut<'a, T>
where
    T: Linked + ?Sized,
{
    pub fn get(&self) -> Option<&'a T> {
        self.current.map(|ptr| unsafe { ptr.as_ref() })
    }
    pub fn get_ptr(&self) -> Link<T> {
        self.current
    }
    pub fn get_mut(&mut self) -> Option<Pin<&'a mut T>> {
        self.current
            .map(|mut ptr| unsafe { Pin::new_unchecked(ptr.as_mut()) })
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
            self.list.len -= 1;

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

            Some(T::from_ptr(self.current?))
        }
    }
}

pub struct Iter<'a, T>
where
    T: Linked + ?Sized,
{
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

impl<'a, T> Iterator for Iter<'a, T>
where
    T: Linked + ?Sized,
{
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

impl<T> ExactSizeIterator for Iter<'_, T>
where
    T: Linked + ?Sized,
{
    fn len(&self) -> usize {
        self.len
    }
}

impl<T> DoubleEndedIterator for Iter<'_, T>
where
    T: Linked + ?Sized,
{
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

pub struct IterMut<'a, T>
where
    T: Linked + ?Sized,
{
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

impl<'a, T> Iterator for IterMut<'a, T>
where
    T: Linked + ?Sized,
{
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

impl<T> ExactSizeIterator for IterMut<'_, T>
where
    T: Linked + ?Sized,
{
    fn len(&self) -> usize {
        self.len
    }
}

impl<T> DoubleEndedIterator for IterMut<'_, T>
where
    T: Linked + ?Sized,
{
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

pub struct IntoIter<T: Linked + ?Sized> {
    list: List<T>,
}

impl<T: Linked + ?Sized> Iterator for IntoIter<T> {
    type Item = T::Handle;

    #[inline]
    fn next(&mut self) -> Option<T::Handle> {
        self.list.pop_front()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.list.len, Some(self.list.len))
    }
}

impl<T: Linked + ?Sized> DoubleEndedIterator for IntoIter<T> {
    #[inline]
    fn next_back(&mut self) -> Option<T::Handle> {
        self.list.pop_back()
    }
}

impl<T: Linked + ?Sized> ExactSizeIterator for IntoIter<T> {
    #[inline]
    fn len(&self) -> usize {
        self.list.len
    }
}

impl<T: Linked + ?Sized> FusedIterator for IntoIter<T> {}

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

    use super::*;
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    use core::mem::offset_of;

    #[derive(Debug)]
    struct TestNode {
        links: Links<TestNode>,
        value: usize,
    }

    impl TestNode {
        fn new(value: usize) -> Pin<Box<Self>> {
            Box::pin(Self {
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
        assert_eq!(v, [0, 1, 2, 3])
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

    #[test]
    fn _drop() {
        let mut list: List<TestNode> = List::new();

        list.push_back(TestNode::new(0));
        list.push_back(TestNode::new(1));
        list.push_back(TestNode::new(2));
        list.push_back(TestNode::new(3));

        drop(list);
    }
}
