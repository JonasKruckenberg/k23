//! Node layout and allocation pools.
//!
//! Each node can hold `B` pivots and `B` values where `B` depends on the
//! [`RangeTreeInteger`] that the tree is based on.
//!
//! Common invariants:
//! - All pivots must be initialized.
//! - `I::MAX` pivots must be at the end, after all other pivots.
//! - Node must have at least `B / 2` elements, except the root node.
//!   - The root node must have at least 2 elements if it is an internal node.
//! - Nodes can have at most `B - 1` elements.
//! - This means that the last pivot is always `I::MAX` and it doesn't hold a
//!   valid value.
//!
//! Notably, it is not a safety violation for non-max pivots to be out of order,
//! although it may cause tree operations to behave incorrectly (but safely).
//!
//! ## Leaf nodes
//!
//! Leaf nodes use a pivot of `I` and a value of `V`.
//!
//! Invariants:
//! - Each pivot is tied to the corresponding value.
//! - A pivot of `I::MAX` indicates that a value is absent.
//!
//! Leaf nodes also hold a `NodeRef` pointing to the next leaf node in the tree.
//! This `NodeRef` overlaps with the last value, which is always unused except
//! temporarily during insertion. The last
//!
//! ## Internal nodes
//!
//! Internal nodes use a pivot of `I` and a value of `NodeRef`.
//!
//! Invariants:
//! - The pivot of a sub-tree indicates the right-most leaf pivot in that sub-tree.
//!   - Safety relies on this being the case even if leaf pivots are out of order.
//! - The last element has a pivot of `I::MAX`.
//!   - All later elements are considered absent.
//!   - Since nodes can only have `B - 1` elements, the last 2 pivots must have a
//!     value of `I::MAX`.

use alloc::alloc::Allocator;
use core::alloc::{AllocError, Layout};
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::slice;

use crate::RangeTreeInteger;

// Maximum allocation size of a `NodePool`. This is used to derive the maximum
// tree height.
#[cfg(target_pointer_width = "64")]
pub(crate) const MAX_POOL_SIZE: usize = u32::MAX as usize;
#[cfg(target_pointer_width = "32")]
pub(crate) const MAX_POOL_SIZE: usize = i32::MAX as usize;

/// A position within a node.
///
/// This has the invariant of always being less than B, which allows safe
/// unchecked indexing within a node.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NodePos<I: RangeTreeInteger> {
    // Using a u32 internally so each stack entry in a cursor is 8 bytes.
    pos: u32,
    marker: PhantomData<fn() -> I>,
}

/// Helper macro to create a `NodePos` at a constant index.
///
/// This is safe since a static assert is used to ensure that `POS` is in
/// bounds.
macro_rules! pos {
    ($expr:expr) => {{
        const { assert!($expr < I::Int::B) };
        #[allow(unused_unsafe, reason = "macro")]
        unsafe {
            $crate::node::NodePos::<I::Int>::new_unchecked($expr)
        }
    }};
}

impl<I: RangeTreeInteger> NodePos<I> {
    /// Returns a `NodePos` pointing to the first element of a node.
    pub(crate) const ZERO: Self = Self {
        pos: 0,
        marker: PhantomData,
    };

    /// Returns a `NodePos` at the given index.
    ///
    /// # Safety
    ///
    /// The index must be less than `I::B`.
    #[inline]
    pub(crate) const unsafe fn new_unchecked(pos: usize) -> Self {
        debug_assert!(pos < I::B);
        Self {
            pos: pos as u32,
            marker: PhantomData,
        }
    }

    /// Returns the position as a `usize`.
    #[inline]
    pub(crate) fn index(self) -> usize {
        self.pos as usize
    }

    /// Returns the position after `self`.
    ///
    /// # Safety
    ///
    /// The resulting index must be less than `B`.
    #[inline]
    pub(crate) unsafe fn next(self) -> Self {
        debug_assert!(self.index() + 1 < I::B);
        Self {
            pos: self.pos + 1,
            marker: PhantomData,
        }
    }

    /// Returns the position before `self`.
    ///
    /// # Safety
    ///
    /// The current index must not be 0.
    #[inline]
    pub(crate) unsafe fn prev(self) -> Self {
        debug_assert_ne!(self.pos, 0);
        Self {
            pos: self.pos - 1,
            marker: PhantomData,
        }
    }

    /// If this position is in the right half of a node, returns the equivalent
    /// position in the destination node after the split.
    #[inline]
    pub(crate) fn split_right_half(self) -> Option<Self> {
        if self.index() >= I::B / 2 {
            Some(Self {
                pos: self.pos - I::B as u32 / 2,
                marker: PhantomData,
            })
        } else {
            None
        }
    }
}

/// A reference to a node inside a `NodePool`.
///
/// This is encoded as a `u32` offset within the pool to save space.
///
/// This doesn't have a lifetime, but is logically bound to the `NodePool` that
/// it was allocated from and is only valid for the lifetime of that pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NodeRef(u32);

impl NodeRef {
    /// Creates a `NodeRef` with a value of 0.
    ///
    /// This is only meant for use when initializing a stack, it is not a valid
    /// `NodeRef`.
    pub(crate) const ZERO: Self = Self(0);

    /// Returns a pointer to the pivot array in the node.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    #[inline]
    unsafe fn pivots_ptr<I: RangeTreeInteger, V>(self, pool: &NodePool<I, V>) -> NonNull<I::Raw> {
        #[cfg(debug_assertions)]
        pool.validate_noderef(self);

        unsafe { pool.ptr.byte_add(self.0 as usize).cast() }
    }

    /// Returns a pointer to the value array in the node.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    #[inline]
    pub(crate) unsafe fn values_ptr<I: RangeTreeInteger, V>(
        self,
        pool: &NodePool<I, V>,
    ) -> NonNull<MaybeUninit<V>> {
        #[cfg(debug_assertions)]
        pool.validate_noderef(self);

        let values_offset = const { node_layout::<I, V>().1 };
        unsafe {
            let ptr = pool.ptr.byte_add(self.0 as usize);
            ptr.byte_add(values_offset).cast::<MaybeUninit<V>>()
        }
    }

    /// Returns the end of the elements of a leaf node.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    #[inline]
    pub(crate) unsafe fn leaf_end<I: RangeTreeInteger, V>(
        self,
        pool: &NodePool<I, V>,
    ) -> NodePos<I> {
        #[cfg(debug_assertions)]
        pool.validate_noderef(self);

        unsafe { I::search(self.pivots(pool), I::MAX) }
    }

    /// Returns the end of the elements of an internal node.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool` and must have at least 1 element.
    #[inline]
    pub(crate) unsafe fn internal_end<I: RangeTreeInteger, V>(
        self,
        pool: &NodePool<I, V>,
    ) -> NodePos<I> {
        #[cfg(debug_assertions)]
        pool.validate_noderef(self);

        unsafe { I::search(self.pivots(pool), I::MAX).next() }
    }

    /// Returns a reference to all the pivots in this node.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    #[inline]
    pub(crate) unsafe fn pivots<I: RangeTreeInteger, V>(self, pool: &NodePool<I, V>) -> &I::Pivots {
        unsafe { self.pivots_ptr(pool).cast::<I::Pivots>().as_ref() }
    }

    /// Returns the pivot at `pos` in this node.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    #[inline]
    pub(crate) unsafe fn pivot<I: RangeTreeInteger, V>(
        self,
        pos: NodePos<I>,
        pool: &NodePool<I, V>,
    ) -> I::Raw {
        unsafe { self.pivots_ptr(pool).add(pos.index()).read() }
    }

    /// Sets the pivot at `pos` in this node.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    #[inline]
    pub(crate) unsafe fn set_pivot<I: RangeTreeInteger, V>(
        self,
        pivot: I::Raw,
        pos: NodePos<I>,
        pool: &mut NodePool<I, V>,
    ) {
        unsafe { self.pivots_ptr(pool).add(pos.index()).write(pivot) }
    }

    /// Returns a reference to the value at `pos` in this node.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    #[inline]
    pub(crate) unsafe fn value<I: RangeTreeInteger, V>(
        self,
        pos: NodePos<I>,
        pool: &NodePool<I, V>,
    ) -> &MaybeUninit<V> {
        unsafe { self.values_ptr(pool).add(pos.index()).as_ref() }
    }

    /// Returns a mutable reference to the value at `pos` in this node.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    #[inline]
    pub(crate) unsafe fn value_mut<I: RangeTreeInteger, V>(
        self,
        pos: NodePos<I>,
        pool: &mut NodePool<I, V>,
    ) -> &mut MaybeUninit<V> {
        unsafe { self.values_ptr(pool).add(pos.index()).as_mut() }
    }

    /// Returns a `NodeRef` pointing to the next leaf node in the tree.
    ///
    /// This overlaps with the last value in the node.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    #[inline]
    pub(crate) unsafe fn next_leaf<I: RangeTreeInteger, V>(
        self,
        pool: &NodePool<I, V>,
    ) -> Option<NodeRef> {
        #[cfg(debug_assertions)]
        pool.validate_noderef(self);

        let next_leaf_offset = const { node_layout::<I, V>().2 };
        let next_leaf = unsafe {
            let ptr = pool.ptr.byte_add(self.0 as usize);
            ptr.byte_add(next_leaf_offset).cast::<NodeRef>().read()
        };
        (next_leaf.0 != !0).then_some(next_leaf)
    }

    /// Sets the `NodeRef` pointing to the next leaf node in the tree.
    ///
    /// This overlaps with the last value in the node.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    #[inline]
    pub(crate) unsafe fn set_next_leaf<I: RangeTreeInteger, V>(
        self,
        next_leaf: Option<NodeRef>,
        pool: &mut NodePool<I, V>,
    ) {
        #[cfg(debug_assertions)]
        pool.validate_noderef(self);

        let next_leaf_offset = const { node_layout::<I, V>().2 };
        unsafe {
            let ptr = pool.ptr.byte_add(self.0 as usize);
            ptr.byte_add(next_leaf_offset)
                .cast::<NodeRef>()
                .write(next_leaf.unwrap_or(NodeRef(!0)));
        }
    }

    /// Inserts `pivot` at `pos`, shifting other pivots up to `node_size` to make
    /// room.
    ///
    /// The pivot previously in the last slot are discarded.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    ///
    /// `node_size` must be greater than `pos.index()` and less than or equal to
    /// `I::B`.
    #[inline]
    pub(crate) unsafe fn insert_pivot<I: RangeTreeInteger, V>(
        self,
        pivot: I::Raw,
        pos: NodePos<I>,
        node_size: usize,
        pool: &mut NodePool<I, V>,
    ) {
        debug_assert!(node_size <= I::B);
        debug_assert!(node_size > pos.index());
        unsafe {
            let ptr = self.pivots_ptr(pool).add(pos.index());
            let count = node_size - pos.index() - 1;
            ptr.copy_to(ptr.add(1), count);
            ptr.write(pivot);
        }
    }

    /// Inserts `value` at `pos`, shifting other values up to `node_size` to
    /// make room.
    ///
    /// The value previously in the last slot are discarded without being
    /// dropped.
    ///
    /// If `node_size == I::B` then this will clobber the next leaf pointer in
    /// the node.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    ///
    /// `node_size` must be greater than `pos.index()` and less than or equal to
    /// `I::B`.
    #[inline]
    pub(crate) unsafe fn insert_value<I: RangeTreeInteger, V>(
        self,
        value: V,
        pos: NodePos<I>,
        node_size: usize,
        pool: &mut NodePool<I, V>,
    ) {
        debug_assert!(node_size <= I::B);
        debug_assert!(node_size > pos.index());
        unsafe {
            let ptr = self.values_ptr(pool).add(pos.index());
            let count = node_size - pos.index() - 1;
            ptr.copy_to(ptr.add(1), count);
            ptr.write(MaybeUninit::new(value));
        }
    }

    /// Removes the pivot at `pos`, shifting other pivots.
    ///
    /// The pivot in the last slot is initialized to `I::MAX`.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    #[inline]
    pub(crate) unsafe fn remove_pivot<I: RangeTreeInteger, V>(
        self,
        pos: NodePos<I>,
        pool: &mut NodePool<I, V>,
    ) {
        unsafe {
            let ptr = self.pivots_ptr(pool).add(pos.index());
            let count = I::B - pos.index() - 1;
            ptr.copy_from(ptr.add(1), count);
            self.pivots_ptr(pool).add(I::B - 1).write(I::MAX);
        }
    }

    /// Removes the value at `pos`, shifting other values.
    ///
    /// The value in the last slot is not modified.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    #[inline]
    pub(crate) unsafe fn remove_value<I: RangeTreeInteger, V>(
        self,
        pos: NodePos<I>,
        pool: &mut NodePool<I, V>,
    ) {
        unsafe {
            let ptr = self.values_ptr(pool).add(pos.index());
            let count = I::B - pos.index() - 1;
            ptr.copy_from(ptr.add(1), count);
        }
    }

    /// Moves the second half of the pivots and values of `self` to `dest`,
    /// replacing the pivots with `I::MAX`.
    ///
    /// The second half of `dest` is initialized with `I::MAX`.
    ///
    /// # Safety
    ///
    /// `self` and `dest` must be allocated from `pool`.
    #[inline]
    pub(crate) unsafe fn split_into<I: RangeTreeInteger, V>(
        self,
        dest: UninitNodeRef,
        pool: &mut NodePool<I, V>,
    ) -> NodeRef {
        unsafe {
            self.pivots_ptr(pool)
                .add(I::B / 2)
                .copy_to_nonoverlapping(dest.0.pivots_ptr(pool), I::B / 2);
            self.values_ptr(pool)
                .add(I::B / 2)
                .copy_to_nonoverlapping(dest.0.values_ptr(pool), I::B / 2);
            slice::from_raw_parts_mut(self.pivots_ptr(pool).add(I::B / 2).as_ptr(), I::B / 2)
                .fill(I::MAX);
            // Make sure not to create a reference to uninitialized memory.
            slice::from_raw_parts_mut(
                dest.0
                    .pivots_ptr(pool)
                    .add(I::B / 2)
                    .cast::<MaybeUninit<I::Raw>>()
                    .as_ptr(),
                I::B / 2,
            )
            .fill(MaybeUninit::new(I::MAX));
        }
        dest.0
    }

    /// Copies the first `count` elements of `src` and appends them to `self` at
    /// `offset`.
    ///
    /// # Safety
    ///
    /// `self` and `src` must be allocated from `pool`.
    ///
    /// `offset.len() + count <= I::B`
    #[inline]
    pub(crate) unsafe fn merge_from<I: RangeTreeInteger, V>(
        self,
        src: NodeRef,
        offset: NodePos<I>,
        count: usize,
        pool: &mut NodePool<I, V>,
    ) {
        unsafe {
            self.pivots_ptr(pool)
                .add(offset.index())
                .copy_from_nonoverlapping(src.pivots_ptr(pool), count);
            self.values_ptr(pool)
                .add(offset.index())
                .copy_from_nonoverlapping(src.values_ptr(pool), count);
        }
    }
}

/// A `NodeRef` pointing to a node whose pivots have not been initialized yet.
///
/// The node must be initialized with `split_into` or `init_pivots`.
#[derive(Debug)]
pub(crate) struct UninitNodeRef(NodeRef);

impl UninitNodeRef {
    /// Initializes all pivots of the node with `I::MAX`.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    #[inline]
    pub(crate) unsafe fn init_pivots<I: RangeTreeInteger, V>(
        self,
        pool: &mut NodePool<I, V>,
    ) -> NodeRef {
        unsafe {
            let ptr = self.0.pivots_ptr(pool).cast::<MaybeUninit<I::Raw>>();
            // Make sure not to create a reference to uninitialized memory.
            let slice = slice::from_raw_parts_mut(ptr.as_ptr(), I::B);
            slice.fill(MaybeUninit::new(I::MAX));
        }
        self.0
    }
}

/// Returns :
/// - the layout of a node with pivot `I` and value `V`.
/// - the offset of the value array within the node.
/// - the offset of the next leaf pointer within the node.
#[inline]
pub(crate) const fn node_layout<I: RangeTreeInteger, V>() -> (Layout, usize, usize) {
    // We require nodes to have at least 4 elements, and the number of elements
    // must be a multiple of 2.
    const { assert!(I::B >= 4) };
    const { assert!(I::B.is_multiple_of(2)) };

    const fn max(a: usize, b: usize) -> usize {
        if a > b { a } else { b }
    }

    // The node layout is effectively:
    // struct Node<I, V> {
    //     pivots: I::pivots, // [I::Raw; I::B]
    //     values: [MaybeUninit<V>; I::B - 1],
    //     last_value: union {
    //          MaybeUninit<V>,
    //          NodeRef,
    //     },
    // }
    //
    // However this can't be expressed directly due to limitations on const
    // generics.
    let pivots = Layout::new::<I::Pivots>();

    let Ok(values) = Layout::array::<V>(I::B - 1) else {
        panic!("Could not calculate node layout");
    };
    let Ok((node, values_offset)) = pivots.extend(values) else {
        panic!("Could not calculate node layout");
    };

    let Ok(last_value) = Layout::from_size_align(
        max(size_of::<V>(), size_of::<NodeRef>()),
        max(align_of::<V>(), align_of::<NodeRef>()),
    ) else {
        panic!("Could not calculate node layout");
    };
    let Ok((node, next_leaf_offset)) = node.extend(last_value) else {
        panic!("Could not calculate node layout");
    };

    // Freed nodes are kept as a linked list of NodeRef, so ensure we can fit a
    // NodeRef in the node.
    let Ok(layout) = node.align_to(4) else {
        panic!("Could not calculate node layout");
    };
    (layout.pad_to_align(), values_offset, next_leaf_offset)
}

/// Memory pool for allocating nodes with pivot `I` and value `V`.
///
/// All nodes are sourced from a single allocation and can therefore be
/// referred to using just a `u32` offset instead of a full pointer.
pub(crate) struct NodePool<I: RangeTreeInteger, V> {
    /// Base of the allocation.
    ptr: NonNull<u8>,

    /// Size of the allocation.
    capacity: u32,

    /// Amount of the allocation currently in use. This is always a multiple of
    /// the node size.
    len: u32,

    /// Linked list of freed nodes, terminated by `!0`.
    free_list: u32,

    marker: PhantomData<(I, V)>,
}

// These impls are needed because `NonNull` suppresses the automatic impls.
unsafe impl<I: RangeTreeInteger + Send, V: Send> Send for NodePool<I, V> {}
unsafe impl<I: RangeTreeInteger + Sync, V: Sync> Sync for NodePool<I, V> {}

impl<I: RangeTreeInteger, V> NodePool<I, V> {
    #[inline]
    pub(crate) fn new() -> Self {
        Self {
            ptr: NonNull::dangling(),
            len: 0,
            capacity: 0,
            free_list: !0,
            marker: PhantomData,
        }
    }

    /// Grows the allocation when it is full.
    ///
    /// This also handles the initial allocation.
    ///
    /// This function is marked `extern "C"` so that any panics in the allocator
    /// cause an abort. This is necessary since the current insert
    /// implementation cannot recover from a failed allocation.
    ///
    /// # Safety
    ///
    /// This pool must always be used with the same allocator.
    fn grow(&mut self, alloc: &impl Allocator) -> Result<(), AllocError> {
        let node_layout = const { node_layout::<I, V>().0 };

        if self.capacity == 0 {
            // Allocate space for 2 nodes for the initial allocation.
            let new_layout = Layout::from_size_align(node_layout.size() * 2, node_layout.align())
                .expect("exceeded RangeTree maximum allocation size");
            assert!(
                new_layout.size() <= MAX_POOL_SIZE,
                "exceeded RangeTree maximum allocation size"
            );
            self.ptr = alloc.allocate(new_layout)?.cast();
            self.capacity = new_layout.size() as u32;
        } else {
            let old_layout = unsafe {
                Layout::from_size_align_unchecked(self.capacity as usize, node_layout.align())
            };

            // This multiplication cannot overflow because the capacity in a
            // layout cannot exceed `isize::MAX`.
            let new_layout =
                Layout::from_size_align(self.capacity as usize * 2, node_layout.align())
                    .expect("exceeded RangeTree maximum allocation size");
            assert!(
                new_layout.size() <= MAX_POOL_SIZE,
                "exceeded RangeTree maximum allocation size"
            );
            self.ptr = unsafe { alloc.grow(self.ptr, old_layout, new_layout)?.cast() };
            self.capacity = new_layout.size() as u32;
        }

        Ok(())
    }

    /// Allocates a new uninitialized node from the pool.
    ///
    /// # Safety
    ///
    /// This pool must always be used with the same allocator.
    #[inline]
    pub(crate) unsafe fn alloc_node(
        &mut self,
        alloc: &impl Allocator,
    ) -> Result<UninitNodeRef, AllocError> {
        let node_layout = const { node_layout::<I, V>().0 };

        // First try re-using a node from the free list.
        if self.free_list != !0 {
            // Freed nodes hold a single `NodeRef` with the next element in the
            // free list.
            let node = UninitNodeRef(NodeRef(self.free_list));
            self.free_list = unsafe { self.ptr.byte_add(self.free_list as usize).cast().read() };
            return Ok(node);
        }

        // Grow the allocation if we've reached capacity.
        if self.len == self.capacity {
            self.grow(alloc)?;
        }

        // grow() will have doubled the capacity or initialized it, which
        // guarantees at least enough space to allocate a single node.
        let node = UninitNodeRef(NodeRef(self.len));
        self.len += node_layout.size() as u32;
        debug_assert!(self.len <= self.capacity);
        Ok(node)
    }

    /// Releases an unused node back to the pool.
    ///
    /// # Safety
    ///
    /// `node` must be allocated from this pool.
    #[inline]
    pub(crate) unsafe fn free_node(&mut self, node: NodeRef) {
        // Just add the node to the linked list of freed nodes.
        unsafe {
            self.ptr
                .byte_add(node.0 as usize)
                .cast()
                .write(self.free_list);
        }
        self.free_list = node.0;
    }

    /// Frees all `NodeRef`s allocated from this pool.
    pub(crate) fn clear(&mut self) {
        self.len = 0;
        self.free_list = !0;
    }

    /// Same as `clear` but then allocates the first node.
    pub(crate) fn clear_and_alloc_node(&mut self) -> UninitNodeRef {
        let node_layout = const { node_layout::<I, V>().0 };
        self.len = node_layout.size() as u32;
        self.free_list = !0;
        UninitNodeRef(NodeRef::ZERO)
    }

    /// Frees the pool and its allocation. This invalidates all `NodeRef`s
    /// allocated from this pool.
    ///
    /// # Safety
    ///
    /// This pool must always be used with the same allocator.
    #[inline]
    pub(crate) unsafe fn clear_and_free(&mut self, alloc: &impl Allocator) {
        self.clear();
        let node_layout = const { node_layout::<I, V>().0 };
        let layout = unsafe {
            Layout::from_size_align_unchecked(self.capacity as usize, node_layout.align())
        };
        unsafe {
            alloc.deallocate(self.ptr, layout);
        }
        self.capacity = 0;
    }

    /// Debug checks to ensure a `NodeRef` is valid.
    #[inline]
    pub(crate) fn validate_noderef(&self, node: NodeRef) {
        let node_layout = const { node_layout::<I, V>().0 };
        assert_eq!(node.0 as usize % node_layout.size(), 0);
        assert!(node.0 < self.len);
    }
}

#[cfg(test)]
mod tests {
    use alloc::alloc::Global;

    use nonmax::NonMaxU32;

    use super::NodePool;

    #[test]
    fn smoke() {
        let mut pool = NodePool::<NonMaxU32, u32>::new();
        let node = unsafe { pool.alloc_node(&Global).unwrap().0 };
        let node2 = unsafe { pool.alloc_node(&Global).unwrap().0 };
        unsafe {
            pool.free_node(node);
        }
        let node3 = unsafe { pool.alloc_node(&Global).unwrap().0 };
        debug_assert_ne!(node, node2);
        debug_assert_eq!(node, node3);
        unsafe {
            pool.clear_and_free(&Global);
        }
    }
}
