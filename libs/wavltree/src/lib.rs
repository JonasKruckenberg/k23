// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

#![cfg_attr(not(test), no_std)]

mod cursor;
mod iter;
mod utils;

use core::borrow::Borrow;
use core::cell::UnsafeCell;
use core::cmp::Ordering;
use core::marker::PhantomPinned;
use core::ops::{Bound, RangeBounds};
use core::pin::Pin;
use core::ptr::NonNull;
use core::{fmt, mem, ptr};

pub use cursor::{Cursor, CursorMut};
pub use iter::{IntoIter, Iter, IterMut};
pub use utils::Side;

use crate::utils::get_sibling;

/// Trait implemented by types which can be members of an [intrusive WAVL tree][WAVLTree].
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
///     links: wavltree::Links<Self>,
///     data: usize,
/// }
/// ```
///
/// The naive implementation of [`links`](Linked::links) for this `Entry` type
/// might look like this:
///
/// ```
/// use wavltree::Linked;
/// use core::ptr::NonNull;
///
/// # struct Entry {
/// #    links: wavltree::Links<Self>,
/// #    data: usize
/// # }
///
/// unsafe impl Linked for Entry {
///     # type Handle = NonNull<Self>;
///     # type Key = usize;
///     # fn get_key(&self) -> &Self::Key { &self.data }
///     # fn into_ptr(r: Self::Handle) -> NonNull<Self> { r }
///     # unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle { ptr }
///     // ...
///
///     unsafe fn links(mut target: NonNull<Self>) -> NonNull<wavltree::Links<Self>> {
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
/// # use wavltree::Linked;
/// # struct Entry {
/// #    links: wavltree::Links<Self>,
/// #    data: usize,
/// # }
///
/// unsafe impl Linked for Entry {
///     # type Handle = NonNull<Self>;
///     # type Key = usize;
///     # fn get_key(&self) -> &Self::Key { &self.data }
///     # fn into_ptr(r: Self::Handle) -> NonNull<Self> { r }
///     # unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle { ptr }
///     // ...
///
///     unsafe fn links(target: NonNull<Self>) -> NonNull<wavltree::Links<Self>> {
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
/// [Stacked Borrows]: https://github.com/rust-lang/unsafe-code-guidelines/blob/master/wip/stacked-borrows.md
pub unsafe trait Linked {
    /// The handle owning nodes in the tree.
    ///
    /// This type must have ownership over a `Self`-typed value. When a `Handle`
    /// is dropped, it should drop the corresponding `Linked` type.
    ///
    /// A quintessential example of a `Handle` is `Box`.
    type Handle;

    /// The type by which entries are identified.
    ///
    /// This type must be a unique identifier of an element, as it is used as the key for all public facing methods (e.g. `[WAVLTree::find`]).
    ///
    /// WAVL trees are sorted meaning that elements must form a total order (entries must be comparable
    /// using `<` and `>`). However, placing the `Ord` requirement directly on entries makes for an
    /// awkward API thanks to the intrusive nature of the data structure, so consumers may define a
    /// custom key type (and key extraction method [`Linked::get_key`]) by which entries are compared.
    ///
    /// # Example
    ///
    /// Suppose this is our element data structure where we want to identify entries *only* by their age.
    ///
    /// ```rust
    /// struct Entry {
    ///     links: wavltree::Links<Self>,
    ///     age: u16,
    ///     name: String
    /// }
    ///
    /// ```
    ///
    /// The corresponding `Linked` implementation would look like this:
    ///
    /// ```rust
    /// # use std::ptr::NonNull;
    ///
    /// # struct Entry {
    /// #    links: wavltree::Links<Self>,
    /// # age: u16,
    /// #    name: String
    /// # }
    ///
    /// unsafe impl wavltree::Linked for Entry {
    ///     # type Handle = NonNull<Self>;
    ///     # fn into_ptr(r: Self::Handle) -> NonNull<Self> { r }
    ///     # unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle { ptr }
    ///     # unsafe fn links(ptr: NonNull<Self>) -> NonNull<wavltree::Links<Entry>> { ptr.map_addr(|a| {
    ///     #    a.checked_add(core::mem::offset_of!(Self, links)).unwrap()
    ///     # }).cast() }
    ///     // ...
    ///
    ///     /// The age is our key
    ///     type Key = u16;
    ///
    ///     /// We just need to retrieve the age from self
    ///     fn get_key(&self) -> &Self::Key {
    ///         &self.age
    ///     }
    /// }
    /// ```
    type Key: Ord;

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

    /// Retrieve the key identifying this node within the collection. See [`Linked::Key`] for details.
    fn get_key(&self) -> &Self::Key;

    /// Invoked on the pivot node, its parent, children, and sibling before a
    /// rotation, just before updating the pointers in the relevant nodes.
    /// The direction of rotation is given by `side`.
    ///
    /// The following diagrams the relationship of the nodes in a left rotation (right rotations are
    /// mirrored):
    ///
    /// ```text
    ///         parent                               self
    ///         /    \                              /    \
    ///     sibling   self        ------->      parent  rl_child
    ///              /    \                      /   \
    ///        lr_child  rl_child           sibling  lr_child
    /// ```
    ///
    /// Note that this hook will be called during double rotations too, once for the opposite side subtree
    /// rotation and once for the final rotation.
    #[allow(unused, reason = "trait declaration")]
    fn after_rotate(
        self: Pin<&mut Self>,
        parent: NonNull<Self>,
        sibling: Link<Self>,
        lr_child: Link<Self>,
        side: Side,
    ) {
    }

    /// Invoked on the node to be erased and the node in the tree where the
    /// augmented invariants become invalid, leading up to the root. Called just
    /// after updating the pointers in the relevant nodes, but before rebalancing.
    #[allow(unused, reason = "trait declaration")]
    fn after_remove(self: Pin<&mut Self>, parent: Link<Self>) {}

    /// Invoked on the newly inserted node before rebalancing.
    fn after_insert(self: Pin<&mut Self>) {}
}

type Link<T> = Option<NonNull<T>>;

/// An intrusive Weak AVL Tree.
///
/// This data structure supports efficient O(log n) lookup of elements and may be used for binary search.
/// All operations complete in logarithmic time.
///
/// A weak AVL Tree (also called WAVL tree) is binary search tree closely related
/// to AVL trees and red-black trees, combining the best properties of both.
/// When built using insertions only it has the same upper height bound of AVL trees (~1.44 log2(n)
/// where n is the number of elements in the tree) while at the same time requiring only a constant
/// number of rotations for insertions and deletions (worst case deletion requires 2 rotations).
pub struct WAVLTree<T>
where
    T: Linked + ?Sized,
{
    pub(crate) root: Link<T>,
}

/// Links to other nodes in a [`WAVLTree`].
///
/// In order to be part of a [`WAVLTree`], a type must contain an instance of this type, and must implement the [`Linked`] trait.
///
/// # Debug assertions
///
/// With debug assertions enabled, `Links` also keeps track of the nodes rank, this is so
/// `WAVLTree::assert_valid` can assert the WAVL rank balancing rules. This increases the size of
/// `Links` by an additional `usize`
pub struct Links<T: ?Sized> {
    inner: UnsafeCell<LinksInner<T>>,
}

struct LinksInner<T: ?Sized> {
    rank_parity: bool,
    up: Link<T>,
    left: Link<T>,
    right: Link<T>,
    #[cfg(debug_assertions)]
    rank: usize,
    /// Linked list links must always be `!Unpin`, in order to ensure that they
    /// never receive LLVM `noalias` annotations; see also
    /// <https://github.com/rust-lang/rust/issues/63818>.
    _unpin: PhantomPinned,
}

// ===== impl WAVLTree =====

unsafe impl<T> Send for WAVLTree<T> where T: Linked + ?Sized {}
unsafe impl<T> Sync for WAVLTree<T> where T: Linked + ?Sized {}

impl<T> Drop for WAVLTree<T>
where
    T: Linked + ?Sized,
{
    fn drop(&mut self) {
        self.clear();
    }
}

impl<T> Default for WAVLTree<T>
where
    T: Linked + ?Sized,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T> WAVLTree<T>
where
    T: Linked + ?Sized,
{
    /// Creates a new, empty tree.
    #[must_use]
    pub const fn new() -> Self {
        Self { root: None }
    }

    /// Returns `true` if the tree contains no entries.
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    pub fn get(&self, key: &T::Key) -> Option<&T> {
        let (node, _) = self.get_internal(key);

        Some(unsafe { node?.as_ref() })
    }

    pub fn get_mut(&self, key: &T::Key) -> Option<Pin<&mut T>> {
        let (node, _) = self.get_internal(key);

        Some(unsafe { Pin::new_unchecked(node?.as_mut()) })
    }

    /// Returns a cursor to the root of the tree.
    #[inline]
    pub fn root(&self) -> Cursor<'_, T> {
        Cursor {
            current: self.root,
            _tree: self,
        }
    }

    /// Returns a mutable cursor to the root of the tree.
    #[inline]
    pub fn root_mut(&mut self) -> CursorMut<'_, T> {
        CursorMut {
            current: self.root,
            _tree: self,
        }
    }

    /// Insert a new entry into the `WAVLTree`.
    ///
    /// # Panics
    ///
    /// Panics if the new entry is already linked to a different intrusive collection.
    pub fn insert(&mut self, element: T::Handle) -> Pin<&mut T> {
        unsafe {
            let mut ptr = T::into_ptr(element);
            debug_assert_ne!(self.root, Some(ptr));

            let ptr_links = T::links(ptr).as_mut();
            assert!(!ptr_links.is_linked());

            let key = T::get_key(ptr.as_ref());

            let was_leaf = if let Some(mut curr) = self.root {
                loop {
                    let curr_links = T::links(curr).as_mut();

                    let side = match key.cmp(curr.as_ref().get_key().borrow()) {
                        Ordering::Equal => panic!("already inserted"),
                        Ordering::Less => Side::Left,
                        Ordering::Greater => Side::Right,
                    };

                    if let Some(child) = curr_links.child(side) {
                        curr = child;
                    } else {
                        let was_leaf = curr_links.is_leaf();
                        ptr_links.replace_parent(Some(curr));
                        curr_links.replace_child(side, Some(ptr));
                        break was_leaf;
                    }
                }
            } else {
                self.root = Some(ptr);
                false
            };

            T::after_insert(Pin::new_unchecked(ptr.as_mut()));

            if was_leaf {
                self.balance_after_insert(ptr);
            }

            Pin::new_unchecked(ptr.as_mut())
        }
    }

    pub fn remove(&mut self, key: &T::Key) -> Option<T::Handle> {
        let (node, _) = self.get_internal(key);

        Some(self.remove_internal(node?))
    }

    /// Removes all elements from the tree.
    ///
    /// This will properly unlink and drop all entries, which requires iterating through the tree.
    pub fn clear(&mut self) {
        if let Some(root) = self.root.take() {
            self.clear_inner(root);
        }
    }

    #[inline]
    #[allow(
        clippy::only_used_in_recursion,
        reason = "need to ensure tree is borrowed for the entire time we operate on it"
    )]
    fn clear_inner(&mut self, node: NonNull<T>) {
        unsafe {
            let node_links = T::links(node).as_mut();
            if let Some(left) = node_links.left() {
                self.clear_inner(left);
            }
            if let Some(right) = node_links.right() {
                self.clear_inner(right);
            }
            node_links.unlink();
            T::from_ptr(node);
        }
    }

    // ===== iteration methods =====

    /// Gets an iterator over the entries in the tree, sorted by their key.
    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            head: self.root.map(|root| utils::find_minimum(root)),
            tail: self.root.map(|root| utils::find_maximum(root)),
            _tree: self,
        }
    }

    /// Gets a mutable iterator over the entries in the tree, sorted by their key.
    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut {
            head: self.root.map(|root| utils::find_minimum(root)),
            tail: self.root.map(|root| utils::find_maximum(root)),
            _tree: self,
        }
    }

    /// Returns a double-ended iterator over a sub-range of entries in the tree.
    ///
    /// The simplest way is to use the range syntax `min..max`, thus `range(min..max)` will yield
    /// elements from min (inclusive) to max (exclusive). The range may also be entered as
    /// `(Bound<T>, Bound<T>)`, so for example `range((Excluded(4), Included(10)))` will yield
    /// a left-exclusive, right-inclusive range from 4 to 10.
    pub fn range<R>(&self, range: R) -> Iter<'_, T>
    where
        R: RangeBounds<T::Key>,
    {
        if self.is_empty() {
            return Iter {
                head: None,
                tail: None,
                _tree: self,
            };
        }

        let start = self.find_lower_bound(range.start_bound());
        let end = self.find_upper_bound(range.end_bound());

        Iter {
            head: start,
            tail: end,
            _tree: self,
        }
    }

    /// Returns a mutable double-ended iterator over a sub-range of entries in the tree.
    ///
    /// The simplest way is to use the range syntax `min..max`, thus `range(min..max)` will yield
    /// elements from min (inclusive) to max (exclusive). The range may also be entered as
    /// `(Bound<T>, Bound<T>)`, so for example `range((Excluded(4), Included(10)))` will yield
    /// a left-exclusive, right-inclusive range from 4 to 10.
    pub fn range_mut<R>(&mut self, range: R) -> IterMut<'_, T>
    where
        R: RangeBounds<T::Key>,
    {
        if self.is_empty() {
            return IterMut {
                head: None,
                tail: None,
                _tree: self,
            };
        }

        let head = self.find_lower_bound(range.start_bound());
        let tail = self.find_upper_bound(range.end_bound());

        IterMut {
            head: head.or(tail),
            tail,
            _tree: self,
        }
    }

    // ===== cursor methods =====

    /// Returns a [`Cursor`] to the first element of the tree.
    pub fn cursor_front(&self) -> Cursor<'_, T> {
        Cursor {
            current: self.root.map(|root| utils::find_minimum(root)),
            _tree: self,
        }
    }

    /// Returns a [`CursorMut`] to the first element of the tree.
    pub fn cursor_front_mut(&mut self) -> CursorMut<'_, T> {
        CursorMut {
            current: self.root.map(|root| utils::find_minimum(root)),
            _tree: self,
        }
    }

    /// Returns a [`Cursor`] to the last element of the tree.
    pub fn cursor_back(&self) -> Cursor<'_, T> {
        Cursor {
            current: self.root.map(|root| utils::find_maximum(root)),
            _tree: self,
        }
    }

    /// Returns a [`CursorMut`] to the last element of the tree.
    #[inline]
    pub fn cursor_back_mut(&mut self) -> CursorMut<'_, T> {
        CursorMut {
            current: self.root.map(|root| utils::find_maximum(root)),
            _tree: self,
        }
    }

    /// Returns a [`Cursor`] pointing at the first element with a key greater
    /// than `bound`.
    ///
    /// Passing `Bound::Included(x)` will return a cursor pointing at the
    /// first element with a key greater than or equal to `x`.
    ///
    /// Passing `Bound::Excluded(x)` will return a cursor pointing at the
    /// first element with a key greater than `x`.
    ///
    /// Passing `Bound::Unbounded` will return a cursor pointing at the
    /// first element in the tree.
    #[inline]
    pub fn lower_bound(&self, bound: Bound<&T::Key>) -> Cursor<'_, T> {
        Cursor {
            current: self.find_lower_bound(bound),
            _tree: self,
        }
    }

    /// Returns a [`CursorMut`] pointing at the first element with a key greater
    /// than `bound`.
    ///
    /// Passing `Bound::Included(x)` will return a cursor pointing at the
    /// first element with a key greater than or equal to `x`.
    ///
    /// Passing `Bound::Excluded(x)` will return a cursor pointing at the
    /// first element with a key greater than `x`.
    ///
    /// Passing `Bound::Unbounded` will return a cursor pointing at the
    /// first element in the tree.
    #[inline]
    pub fn lower_bound_mut(&mut self, bound: Bound<&T::Key>) -> CursorMut<'_, T> {
        CursorMut {
            current: self.find_lower_bound(bound),
            _tree: self,
        }
    }

    /// Returns a [`Cursor`] pointing at the last element with a key less
    /// than `bound`.
    ///
    /// Passing `Bound::Included(x)` will return a cursor pointing at the
    /// last element with a key lesser than or equal to `x`.
    ///
    /// Passing `Bound::Excluded(x)` will return a cursor pointing at the
    /// last element with a key lesser than `x`.
    ///
    /// Passing `Bound::Unbounded` will return a cursor pointing at the
    /// last element in the tree.
    #[inline]
    pub fn upper_bound(&self, bound: Bound<&T::Key>) -> Cursor<'_, T> {
        Cursor {
            current: self.find_upper_bound(bound),
            _tree: self,
        }
    }

    /// Returns a [`CursorMut`] pointing at the last element with a key less
    /// than `bound`.
    ///
    /// Passing `Bound::Included(x)` will return a cursor pointing at the
    /// last element with a key lesser than or equal to `x`.
    ///
    /// Passing `Bound::Excluded(x)` will return a cursor pointing at the
    /// last element with a key lesser than `x`.
    ///
    /// Passing `Bound::Unbounded` will return a cursor pointing at the
    /// last element in the tree.
    #[inline]
    pub fn upper_bound_mut(&mut self, bound: Bound<&T::Key>) -> CursorMut<'_, T> {
        CursorMut {
            current: self.find_upper_bound(bound),
            _tree: self,
        }
    }

    // ===== utility methods =====

    /// Asserts as many of the tree's invariants as possible.
    ///
    /// Note that with debug assertions enabled, this includes validating the WAVL rank-balancing
    /// rules **which is disabled otherwise**.
    #[track_caller]
    pub fn assert_valid(&self, ctx: &str) {
        unsafe {
            if let Some(root) = self.root {
                let root_links = T::links(root).as_ref();
                root_links.assert_valid(ctx);

                if let Some(left) = root_links.left() {
                    Self::assert_valid_inner(left, root, ctx);
                }

                if let Some(right) = root_links.right() {
                    Self::assert_valid_inner(right, root, ctx);
                }
            }
        }
    }

    #[track_caller]
    #[cfg_attr(not(debug_assertions), allow(unused))]
    fn assert_valid_inner(node: NonNull<T>, parent: NonNull<T>, ctx: &str) {
        unsafe {
            let node_links = T::links(node).as_ref();

            // assert that all links are set up correctly (no loops, self references, etc.)
            node_links.assert_valid(ctx);

            // We can only check the WAVL rule if we track the rank, which we only do in debug builds
            #[cfg(debug_assertions)]
            {
                let parent_links = T::links(parent).as_ref();

                // Weak AVL Rule: All rank differences are 1 or 2 and every leaf has rank 0.
                let rank_diff = parent_links.rank() - node_links.rank();
                assert!(
                    rank_diff <= 2 && rank_diff > 0,
                    "{ctx}WAVL rank rule violation: rank difference must be 1 or 2, but was {rank_diff}; node = {node:#?}, parent = {parent:#?}",
                );
                if node_links.is_leaf() {
                    assert_eq!(
                        node_links.rank(),
                        0,
                        "{ctx}WAVL rank rule violation: leaf must be rank 0, but was {}",
                        node_links.rank(),
                    );
                }
            }

            if let Some(left) = node_links.left() {
                // Assert that values in the right subtree are indeed less
                assert!(
                    left.as_ref().get_key() < node.as_ref().get_key(),
                    "{ctx}Ordering violation: left subtree is not less than node"
                );
                Self::assert_valid_inner(left, node, ctx);
            }

            if let Some(right) = node_links.right() {
                // Assert that values in the right subtree are indeed greater
                assert!(
                    right.as_ref().get_key() > node.as_ref().get_key(),
                    "{ctx}Ordering violation: right subtree is not greater than node"
                );
                Self::assert_valid_inner(right, node, ctx);
            }
        }
    }

    // ===== internal methods =====

    fn find_lower_bound(&self, bound: Bound<&T::Key>) -> Option<NonNull<T>> {
        let mut result = None;
        let mut tree = self.root;
        while let Some(curr) = tree {
            let curr_lks = unsafe { T::links(curr).as_ref() };

            let cond = match bound {
                Bound::Included(key) => key <= unsafe { curr.as_ref() }.get_key().borrow(),
                Bound::Excluded(key) => key < unsafe { curr.as_ref() }.get_key().borrow(),
                Bound::Unbounded => true,
            };

            if cond {
                result = tree;
                tree = curr_lks.left();
            } else {
                tree = curr_lks.right();
            }
        }

        result
    }

    fn find_upper_bound(&self, bound: Bound<&T::Key>) -> Option<NonNull<T>> {
        let mut result = None;
        let mut tree = self.root;
        while let Some(curr) = tree {
            let curr_lks = unsafe { T::links(curr).as_ref() };

            let cond = match bound {
                Bound::Included(key) => key < unsafe { curr.as_ref() }.get_key().borrow(),
                Bound::Excluded(key) => key <= unsafe { curr.as_ref() }.get_key().borrow(),
                Bound::Unbounded => false,
            };

            if cond {
                tree = curr_lks.left();
            } else {
                result = tree;
                tree = curr_lks.right();
            }
        }

        result
    }

    #[expect(clippy::type_complexity, reason = "internal")]
    fn get_internal(&self, key: &T::Key) -> (Option<NonNull<T>>, Option<(NonNull<T>, Side)>) {
        let mut parent = None;
        let mut tree = self.root;
        while let Some(curr) = tree {
            let curr_lks = unsafe { T::links(curr).as_ref() };

            match key.cmp(unsafe { curr.as_ref() }.get_key().borrow()) {
                Ordering::Equal => return (Some(curr), parent),
                Ordering::Less => {
                    parent = Some((curr, Side::Left));
                    tree = curr_lks.left();
                }
                Ordering::Greater => {
                    parent = Some((curr, Side::Right));
                    tree = curr_lks.right();
                }
            }
        }

        (None, parent)
    }

    fn remove_internal(&mut self, mut node: NonNull<T>) -> T::Handle {
        let node_links = unsafe { T::links(node).as_mut() };
        let parent = node_links.parent();

        // Figure out which node we need to splice in, replacing node
        let y = if let Some(right) = node_links.right()
            && node_links.left().is_some()
        {
            utils::find_minimum(right)
        } else {
            node
        };

        // Find the child if the node to that we will move up
        let y_links = unsafe { T::links(y).as_ref() };
        let mut p_y = y_links.parent();
        let x = y_links.left().or(y_links.right());

        // Check if y is a 2-child of its parent
        let is_2_child = p_y.is_some_and(|parent| utils::node_is_2_child(y, parent));

        // Replace Y with X which will effectively remove Y from the tree
        {
            if let Some(p_y) = y_links.parent() {
                let p_y_links = unsafe { T::links(p_y).as_mut() };

                // Ensure the right/left pointer of the parent of the node to
                // be spliced out points to its new child
                if p_y_links.left() == Some(y) {
                    p_y_links.replace_left(x);
                } else {
                    assert_eq!(p_y_links.right(), Some(y));
                    p_y_links.replace_right(x);
                }
            } else {
                // We're deleting the root, so swap in the new candidate
                self.root = x;
            }

            // Splice in the child of the node to be removed
            if let Some(x) = x {
                unsafe { T::links(x).as_mut() }.replace_parent(y_links.parent());
            }
        }

        if !ptr::eq(y.as_ptr(), node.as_ptr()) {
            self.swap_in_node_at(node, y);
            if p_y == Some(node) {
                p_y = Some(y);
            }
        }

        T::after_remove(unsafe { Pin::new_unchecked(node.as_mut()) }, parent);

        if let Some(p_y) = p_y {
            if is_2_child {
                self.rebalance_after_remove_3_child(x, p_y);
            } else if x.is_none() && unsafe { T::links(p_y).as_ref() }.is_leaf() {
                self.rebalance_after_remove_2_2_leaf(p_y);
            }

            assert!(
                !(unsafe { T::links(p_y).as_ref() }.is_leaf()
                    && unsafe { T::links(p_y).as_ref() }.rank_parity())
            );
        }

        // unlink the node from the tree and return
        unsafe {
            node_links.unlink();
            T::from_ptr(node)
        }
    }

    pub(crate) fn balance_after_insert(&mut self, mut x: NonNull<T>) {
        unsafe {
            let mut parent = T::links(x).as_ref().parent().unwrap();

            // The WAVL rank rules require all rank differences to be either 1 or 2; 0 is now allowed.
            // The parent was previously a 1,1 leaf, but is now a 0,1 unary node. 0 is not allowed
            // so we need to rebalance.
            //
            // Sep 1: Promotion
            // We begin with promoting the parent nodes, according to the following algorithm:
            //
            // while parent_.is_some() && parent is 0,1
            //      promote parent
            //      move up the tree
            //
            // To determine whether parent is a 0,1 node, we need `curr`s rank parity,
            // `parent`s rank parity and the other sibling's parity which we read below.
            // Note, that they are all `let mut` because we need to update them each iteration.

            let mut par_x: bool;
            let mut par_parent: bool;
            let mut par_sibling: bool;
            let mut sibling_side: Side;

            loop {
                // promote
                let parent_links = T::links(parent).as_mut();
                parent_links.promote();

                let Some(parent_) = parent_links.parent() else {
                    return;
                };

                // climb
                x = parent;
                parent = parent_;

                // update parities
                // note that we explicitly create new `T::links` references here bc we just updated the pointers.
                par_x = T::links(x).as_ref().rank_parity();
                par_parent = T::links(parent).as_ref().rank_parity();

                let (sibling, side) = utils::get_sibling(Some(x), parent);
                par_sibling = utils::get_link_parity(sibling);
                sibling_side = side;

                // Let N, P and S denote the current node, parent, and sibling parities
                // that we read above. Then `parent` is 0,1 iff (!N * !P * S) + (N * P * !S)
                //
                // This means when the inverse is true, we reached a parent that's not 0,1 and so
                // we can stop the promotion loop.
                if (!par_x || !par_parent || par_sibling) && (par_x || par_parent || !par_sibling) {
                    break;
                }
            }

            // At this point we know `x` has a parent and that parent is not 0,1. So either,
            // the rank rule has been restored or the parent is 0,2.
            //
            // Using the notation above, our parent is 0,2 iff (!N * !P * !S) + (N * P * S).
            // The inverse can be expressed much more succinctly as (N != P) || (N != S)
            // (according to godbolt also generates 3x less code)
            //
            // Therefore, iff (N != P) || (N != S) the rank rule holds and we are done
            if (par_x != par_parent) || (par_x != par_sibling) {
                return;
            }

            let x_links = T::links(x).as_mut();
            debug_assert!(x_links.parent().is_some());

            // If X is a left child, we rotate right, if it's a right child we rotate left
            //
            // We define
            // - Y as X's child in direction of rotation
            // - Z as X's parent
            let y = x_links.child(sibling_side);
            let z = x_links.parent();

            if let Some(y) = y
                && T::links(y).as_ref().rank_parity() != par_x
            {
                // If Y is a 1-child we do a double rotation, then demote x and z
                self.double_rotate_at(y, sibling_side);

                T::links(y).as_mut().promote();
                x_links.demote();
            } else {
                // If not, do a single rotation and demote z
                self.rotate_at(x, sibling_side);
            }

            // finish up by doing the z demotion
            if let Some(z) = z {
                T::links(z).as_mut().demote();
            }
        }
    }

    fn rebalance_after_remove_3_child(&mut self, mut x: Link<T>, mut z: NonNull<T>) {
        let mut z_links = unsafe { T::links(z).as_mut() };

        // Step 1: Demotions.
        //
        // The paper states "While X is 3-child and Y is a 2-child or 2,2"
        loop {
            let y = if z_links.left() == x {
                z_links.right()
            } else {
                z_links.left()
            };

            let creates_3_node = z_links.parent().is_some_and(|p_z| {
                unsafe { T::links(p_z).as_ref() }.rank_parity() == z_links.rank_parity()
            });

            if utils::get_link_parity(y) == z_links.rank_parity() {
                z_links.demote();
            } else {
                let y_links = unsafe { T::links(y.unwrap()).as_mut() };

                // compute y_is_22_node
                let y_is_22_node = if y_links.rank_parity() {
                    // If Y has odd rank parity, it is a 2,2 node if both its
                    // children have odd parity, meaning each child either does
                    // not exist, or exists and has odd parity.
                    utils::get_link_parity(y_links.left())
                        && utils::get_link_parity(y_links.right())
                } else {
                    // If Y has even rank parity, it can only be a 2,2 node if it is
                    // a binary node and both of its children have even parity.
                    let y_left_links = y_links.left().map(|l| unsafe { T::links(l).as_ref() });
                    let y_right_links = y_links.right().map(|r| unsafe { T::links(r).as_ref() });

                    y_left_links.is_some_and(|l| !l.rank_parity())
                        && y_right_links.is_some_and(|l| !l.rank_parity())
                };

                if y_is_22_node {
                    y_links.demote();
                    z_links.demote();
                } else {
                    // At this point we know that y is neither a 2-child nor a 2,2 node
                    // and give the loop conditions above this means we're done with promotions.
                    // x still might be a 3-child, but that will be fixed with rotations below.
                    break;
                }
            }

            if let Some(parent) = z_links.parent() {
                // climbing up
                x = Some(z);
                z = parent;
                z_links = unsafe { T::links(z).as_mut() };
            } else {
                // we reached the root so were done rebalancing
                return;
            }

            if !creates_3_node {
                return;
            }
        }

        // Step 2: Rotation
        let (y, y_side) = get_sibling(x, z);
        let y_links = unsafe { T::links(y.unwrap()).as_mut() };

        let v = y_links.child(y_side.opposite());
        let w = y_links.child(y_side);

        if utils::get_link_parity(w) != y_links.rank_parity() {
            self.rotate_at(y.unwrap(), y_side.opposite());

            y_links.promote();
            z_links.demote();

            if z_links.is_leaf() {
                z_links.demote();
            }
        } else {
            let v = v.unwrap();
            let v_links = unsafe { T::links(v).as_mut() };

            self.double_rotate_at(v, y_side.opposite());

            v_links.double_promote();
            y_links.demote();
            z_links.double_demote();
        }
    }

    fn rebalance_after_remove_2_2_leaf(&mut self, x: NonNull<T>) {
        // If we just turned node into a 2,2 leaf, it will have no children and
        // odd rank-parity.
        let x_links = unsafe { T::links(x).as_mut() };

        if !x_links.rank_parity() || x_links.left().is_some() || x_links.right().is_some() {
            return;
        }

        if let Some(parent) = x_links.parent()
            && crate::utils::node_is_2_child(x, parent)
        {
            // Demote the node turning it into a 1,1 leaf.
            x_links.demote();

            // By demoting this node, we just created a 3-child so we need to deal with that.
            self.rebalance_after_remove_3_child(Some(x), parent);
        } else {
            // Demote the node turning it into a 1,1 leaf.
            x_links.demote();
        }
    }

    fn rotate_at(&mut self, mut x: NonNull<T>, side: Side) {
        let x_links = unsafe { T::links(x).as_mut() };
        let y = x_links.child(side);
        let z = x_links.parent().unwrap();
        let z_links = unsafe { T::links(z).as_mut() };
        let p_z = z_links.parent();

        T::after_rotate(
            unsafe { Pin::new_unchecked(x.as_mut()) },
            z,
            get_sibling(Some(x), z).0,
            y,
            side,
        );

        // Rotate X into place
        x_links.replace_parent(p_z);
        if let Some(p_z) = p_z {
            let p_z_links = unsafe { T::links(p_z).as_mut() };

            if p_z_links.left() == Some(z) {
                p_z_links.replace_left(Some(x));
            } else {
                p_z_links.replace_right(Some(x));
            }
        } else {
            self.root = Some(x);
        }

        // make z the `side`-child of x
        x_links.replace_child(side, Some(z));
        z_links.replace_parent(Some(x));

        // make y the `opposite side`-child of z
        z_links.replace_child(side.opposite(), y);
        if let Some(y) = y {
            unsafe { T::links(y).as_mut() }.replace_parent(Some(z));
        }
    }

    fn double_rotate_at(&mut self, mut y: NonNull<T>, side: Side) {
        let y_links = unsafe { T::links(y).as_mut() };

        let x = y_links.parent().unwrap();
        let x_links = unsafe { T::links(x).as_ref() };
        let z = x_links.parent().unwrap();
        let z_links = unsafe { T::links(z).as_ref() };
        let p_z = z_links.parent();

        T::after_rotate(
            unsafe { Pin::new_unchecked(y.as_mut()) },
            x,
            get_sibling(Some(y), x).0,
            y_links.child(side.opposite()),
            side.opposite(),
        );
        T::after_rotate(
            unsafe { Pin::new_unchecked(y.as_mut()) },
            z,
            get_sibling(Some(x), z).0,
            y_links.child(side),
            side,
        );

        // Rotate Y into place
        y_links.replace_parent(p_z);
        if let Some(p_z) = p_z {
            let p_z_links = unsafe { T::links(p_z).as_mut() };

            if p_z_links.left() == Some(z) {
                p_z_links.replace_left(Some(y));
            } else {
                p_z_links.replace_right(Some(y));
            }
        } else {
            self.root = Some(y);
        }

        let mut move_subtrees = |lt: NonNull<T>, gt: NonNull<T>| {
            let lt_links = unsafe { T::links(lt).as_mut() };
            let gt_links = unsafe { T::links(gt).as_mut() };

            // Move y's left subtree (since lt > left(y)) to lt's right subtree
            lt_links.replace_right(y_links.left());

            if let Some(left) = y_links.left() {
                unsafe { T::links(left).as_mut() }.replace_parent(Some(lt));
            }

            y_links.replace_left(Some(lt));
            lt_links.replace_parent(Some(y));

            // Move y's right subtree (since gt > right(y)) to gt's left subtree
            gt_links.replace_left(y_links.right());

            if let Some(right) = y_links.right() {
                unsafe { T::links(right).as_mut() }.replace_parent(Some(gt));
            }

            y_links.replace_right(Some(gt));
            gt_links.replace_parent(Some(y));
        };

        match side {
            Side::Left => move_subtrees(z, x),
            Side::Right => move_subtrees(x, z),
        }
    }

    fn swap_in_node_at(&mut self, old: NonNull<T>, new: NonNull<T>) {
        let old_links = unsafe { T::links(old).as_mut() };
        let new_links = unsafe { T::links(new).as_mut() };

        let parent = old_links.parent();
        let left = old_links.left();
        let right = old_links.right();

        new_links.replace_parent(parent);
        if let Some(parent) = parent {
            let parent_links = unsafe { T::links(parent).as_mut() };
            if parent_links.left() == Some(old) {
                parent_links.replace_left(Some(new));
            } else {
                parent_links.replace_right(Some(new));
            }
        } else {
            self.root = Some(new);
        }

        new_links.replace_left(left);
        if let Some(left) = left {
            unsafe { T::links(left).as_mut() }.replace_parent(Some(new));
        }
        old_links.replace_left(None);

        new_links.replace_right(right);
        if let Some(right) = right {
            unsafe { T::links(right).as_mut() }.replace_parent(Some(new));
        }
        old_links.replace_right(None);

        // update parity
        new_links.set_rank(old_links);

        old_links.replace_parent(None);
    }
}

impl<T> IntoIterator for WAVLTree<T>
where
    T: Linked + ?Sized,
{
    type Item = T::Handle;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        #[allow(if_let_rescope, reason = "")]
        if let Some(root) = self.root {
            IntoIter {
                // TODO this could be optimized by caching the head and tail nodes in the WAVLTree
                head: Some(utils::find_minimum(root)),
                tail: Some(utils::find_maximum(root)),
                _tree: self,
            }
        } else {
            IntoIter {
                head: None,
                tail: None,
                _tree: self,
            }
        }
    }
}

impl<'a, T: Linked + ?Sized> IntoIterator for &'a WAVLTree<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T: Linked + ?Sized> IntoIterator for &'a mut WAVLTree<T> {
    type Item = Pin<&'a mut T>;
    type IntoIter = IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

// ===== impl Links =====

impl<T: ?Sized> Default for Links<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: ?Sized> fmt::Debug for Links<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut f = f.debug_struct("Links");

        f.field("self", &format_args!("{self:p}"))
            .field("rank_parity", &self.rank_parity())
            .field("parent", &self.parent())
            .field("left", &self.left())
            .field("right", &self.left());

        #[cfg(debug_assertions)]
        f.field("rank", &self.rank());

        f.finish()
    }
}

impl<T: ?Sized> Links<T> {
    /// Returns new links for a [Weak AVL tree][WAVLTree].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(LinksInner {
                rank_parity: false, // nodes start out as leaves with rank 0, even parity
                #[cfg(debug_assertions)]
                rank: 0,
                up: None,
                left: None,
                right: None,
                _unpin: PhantomPinned,
            }),
        }
    }

    /// Returns `true` if this node is currently linked to a [WAVLTree].
    pub const fn is_linked(&self) -> bool {
        let inner = unsafe { &*self.inner.get() };
        inner.up.is_some() || inner.left.is_some() || inner.right.is_some()
    }

    /// Forcefully unlinks this node from the tree.
    ///
    /// # Safety
    ///
    /// Calling this method on a node that is linked to a tree, **will corrupt the tree** leaving
    /// pointers to arbitrary memory around.
    const unsafe fn unlink(&mut self) {
        self.inner.get_mut().up = None;
        self.inner.get_mut().left = None;
        self.inner.get_mut().right = None;
        self.inner.get_mut().rank_parity = false;
    }

    #[inline]
    pub const fn parent(&self) -> Link<T> {
        unsafe { (*self.inner.get()).up }
    }
    #[inline]
    pub const fn left(&self) -> Link<T> {
        unsafe { (*self.inner.get()).left }
    }
    #[inline]
    pub const fn right(&self) -> Link<T> {
        unsafe { (*self.inner.get()).right }
    }

    #[inline]
    pub const fn replace_parent(&mut self, lk: Link<T>) -> Link<T> {
        mem::replace(&mut self.inner.get_mut().up, lk)
    }
    #[inline]
    pub const fn replace_left(&mut self, lk: Link<T>) -> Link<T> {
        mem::replace(&mut self.inner.get_mut().left, lk)
    }
    #[inline]
    pub const fn replace_right(&mut self, lk: Link<T>) -> Link<T> {
        mem::replace(&mut self.inner.get_mut().right, lk)
    }

    #[inline]
    #[cfg(debug_assertions)]
    const fn rank(&self) -> usize {
        unsafe { (*self.inner.get()).rank }
    }
    #[inline]
    const fn rank_parity(&self) -> bool {
        unsafe { (*self.inner.get()).rank_parity }
    }
    // TODO test
    #[inline]
    const fn promote(&mut self) {
        self.inner.get_mut().rank_parity = !self.rank_parity();
        #[cfg(debug_assertions)]
        {
            self.inner.get_mut().rank += 1;
        }
    }
    // TODO test
    #[inline]
    const fn demote(&mut self) {
        self.inner.get_mut().rank_parity = !self.rank_parity();
        #[cfg(debug_assertions)]
        {
            self.inner.get_mut().rank -= 1;
        }
    }
    #[inline]
    const fn double_promote(&mut self) {
        #[cfg(debug_assertions)]
        {
            self.inner.get_mut().rank += 2;
        }
    }
    #[inline]
    const fn double_demote(&mut self) {
        #[cfg(debug_assertions)]
        {
            self.inner.get_mut().rank -= 2;
        }
    }
    const fn set_rank(&mut self, other: &Self) {
        self.inner.get_mut().rank_parity = other.rank_parity();
        #[cfg(debug_assertions)]
        {
            self.inner.get_mut().rank = other.rank();
        }
    }

    pub const fn is_leaf(&self) -> bool {
        self.left().is_none() && self.right().is_none()
    }

    #[inline]
    const fn child(&self, side: Side) -> Link<T> {
        match side {
            Side::Left => unsafe { (*self.inner.get()).left },
            Side::Right => unsafe { (*self.inner.get()).right },
        }
    }
    #[inline]
    const fn replace_child(&mut self, side: Side, child: Link<T>) -> Link<T> {
        match side {
            Side::Left => mem::replace(&mut self.inner.get_mut().left, child),
            Side::Right => mem::replace(&mut self.inner.get_mut().right, child),
        }
    }

    /// Asserts as many invariants about this particular node as possible.
    ///
    /// # Panics
    ///
    /// Panics when an assertion does not hold.
    #[track_caller]
    pub fn assert_valid(&self, ctx: &str)
    where
        T: Linked,
    {
        if let Some(parent) = self.parent() {
            assert_ne!(
                unsafe { T::links(parent) },
                NonNull::from(self),
                "{ctx}node's parent cannot be itself; node={self:#?}"
            );
        }

        if let Some(left) = self.left() {
            assert_ne!(
                unsafe { T::links(left) },
                NonNull::from(self),
                "{ctx}node's left child cannot be itself; node={self:#?}"
            );
        }

        if let Some(right) = self.right() {
            assert_ne!(
                unsafe { T::links(right) },
                NonNull::from(self),
                "{ctx}node's right child cannot be itself; node={self:#?}"
            );
        }
        if let (Some(parent), Some(left)) = (self.parent(), self.left()) {
            assert_ne!(
                unsafe { T::links(parent) },
                unsafe { T::links(left) },
                "{ctx}node's parent and left child cannot be the same; node={self:#?}"
            );
        }
        if let (Some(parent), Some(right)) = (self.parent(), self.right()) {
            assert_ne!(
                unsafe { T::links(parent) },
                unsafe { T::links(right) },
                "{ctx}node's parent and right child cannot be the same; node={self:#?}"
            );
        }
        if let (Some(left), Some(right)) = (self.left(), self.right()) {
            assert_ne!(
                unsafe { T::links(left) },
                unsafe { T::links(right) },
                "{ctx}node's left and right children cannot be the same; node={self:#?}"
            );
        }
    }
}
