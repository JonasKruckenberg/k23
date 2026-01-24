use core::alloc::{AllocError, Allocator, Layout};
use core::fmt::Debug;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::slice;

use crate::int::RangeTreeInteger;

// Maximum allocation size of a `NodePool`. This is used to derive the maximum
// tree height.
#[cfg(target_pointer_width = "64")]
pub(crate) const MAX_POOL_SIZE: usize = u32::MAX as usize;
#[cfg(target_pointer_width = "32")]
pub(crate) const MAX_POOL_SIZE: usize = i32::MAX as usize;

#[derive(Clone, Copy, Debug)]
pub struct NodePos<I: RangeTreeInteger> {
    pos: u8,
    _m: PhantomData<fn() -> I>,
}

macro_rules! pos {
    ($expr:expr) => {{
        const { assert!($expr < I::Int::B) };
        #[allow(unused_unsafe)]
        unsafe {
            $crate::node::NodePos::<I::Int>::new_unchecked($expr)
        }
    }};
}
pub(crate) use pos;

impl<I: RangeTreeInteger> NodePos<I> {
    pub const ZERO: Self = Self {
        pos: 0,
        _m: PhantomData,
    };

    #[inline]
    pub(crate) const unsafe fn new_unchecked(pos: usize) -> Self {
        debug_assert!(pos < I::B);
        Self {
            pos: pos as u8,
            _m: PhantomData,
        }
    }

    /// Returns the position as a `usize`.
    #[inline]
    pub(crate) fn index(self) -> usize {
        self.pos as usize
    }

    #[inline]
    pub(crate) unsafe fn next(self) -> Self {
        debug_assert!(self.index() + 1 < I::B);
        Self {
            pos: self.pos + 1,
            _m: PhantomData,
        }
    }

    #[inline]
    pub(crate) unsafe fn prev(self) -> Self {
        debug_assert_ne!(self.pos, 0);
        Self {
            pos: self.pos - 1,
            _m: PhantomData,
        }
    }

    /// If this position is in the right half of a node, returns the equivalent
    /// position in the destination node after the split.
    #[inline]
    pub(crate) fn split_right_half(self) -> Option<Self> {
        if self.index() >= I::B / 2 {
            Some(Self {
                pos: self.pos - I::B as u8 / 2,
                _m: PhantomData,
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
    pub(crate) const ZERO: Self = Self(0);

    #[inline]
    unsafe fn pivots_ptr<I: RangeTreeInteger, Type>(
        self,
        pool: &NodePool<I, Type>,
    ) -> NonNull<I::Raw> {
        #[cfg(debug_assertions)]
        self.assert_valid(pool);

        unsafe { pool.ptr.byte_add(self.0 as usize).cast::<I::Raw>() }
    }

    #[inline]
    pub(crate) unsafe fn pivots<I: RangeTreeInteger, Type>(
        self,
        pool: &NodePool<I, Type>,
    ) -> &I::Pivots {
        unsafe { self.pivots_ptr(pool).cast::<I::Pivots>().as_ref() }
    }

    #[inline]
    pub(crate) unsafe fn pivot<I: RangeTreeInteger, Type>(
        self,
        pos: NodePos<I>,
        pool: &NodePool<I, Type>,
    ) -> I::Raw {
        unsafe { self.pivots_ptr(pool).add(pos.index()).read() }
    }

    #[inline]
    pub(crate) unsafe fn set_pivot<I: RangeTreeInteger, Type>(
        self,
        pivot: I::Raw,
        pos: NodePos<I>,
        pool: &NodePool<I, Type>,
    ) {
        unsafe { self.pivots_ptr(pool).add(pos.index()).write(pivot) };
    }

    #[inline]
    pub(crate) unsafe fn insert_pivot<I: RangeTreeInteger, Type>(
        self,
        key: I::Raw,
        pos: NodePos<I>,
        node_size: usize,
        pool: &mut NodePool<I, Type>,
    ) {
        debug_assert!(node_size <= I::B);
        debug_assert!(node_size > pos.index());
        unsafe {
            let ptr = self.pivots_ptr(pool).add(pos.index());
            let count = node_size - pos.index() - 1;
            ptr.copy_to(ptr.add(1), count);
            ptr.write(key);
        }
    }

    #[inline]
    pub(crate) fn assert_valid<I: RangeTreeInteger, Type>(&self, pool: &NodePool<I, Type>) {
        // let node_layout = const { node_layout::<I, V>().0 };
        // debug_assert_eq!(node.0 as usize % node_layout.size(), 0);
        assert!(self.0 < pool.len);
    }
}

impl NodeRef {
    #[inline]
    unsafe fn children_ptr<I: RangeTreeInteger>(
        self,
        pool: &NodePool<I, marker::Internal>,
    ) -> NonNull<MaybeUninit<NodeRef>> {
        let (_, children_offset) = const { internal_node_layout::<I>() };
        unsafe {
            let ptr = pool.ptr.byte_add(self.0 as usize);
            ptr.byte_add(children_offset).cast::<MaybeUninit<NodeRef>>()
        }
    }

    #[inline]
    pub(crate) unsafe fn child<I: RangeTreeInteger>(
        self,
        pos: NodePos<I>,
        pool: &NodePool<I, marker::Internal>,
    ) -> &MaybeUninit<NodeRef> {
        unsafe { self.children_ptr(pool).add(pos.index()).as_ref() }
    }

    #[inline]
    pub(crate) unsafe fn child_mut<I: RangeTreeInteger>(
        self,
        pos: NodePos<I>,
        pool: &mut NodePool<I, marker::Internal>,
    ) -> &mut MaybeUninit<NodeRef> {
        unsafe { self.children_ptr(pool).add(pos.index()).as_mut() }
    }

    #[inline]
    pub(crate) unsafe fn insert_child<I: RangeTreeInteger>(
        self,
        child: NodeRef,
        pos: NodePos<I>,
        node_size: usize,
        pool: &mut NodePool<I, marker::Internal>,
    ) {
        debug_assert!(node_size <= I::B);
        debug_assert!(node_size > pos.index());
        unsafe {
            let ptr = self.children_ptr(pool).add(pos.index());
            let count = node_size - pos.index() - 1;
            ptr.copy_to(ptr.add(1), count);
            ptr.write(MaybeUninit::new(child));
        }
    }

    #[inline]
    pub(crate) unsafe fn internal_split_into<I: RangeTreeInteger>(
        self,
        dest: UninitNodeRef,
        pool: &mut NodePool<I, marker::Internal>,
    ) -> Self {
        unsafe {
            // copy the second half of our node into dest
            self.pivots_ptr(pool)
                .add(I::B / 2)
                .copy_to_nonoverlapping(dest.0.pivots_ptr(pool), I::B / 2);
            self.children_ptr(pool)
                .add(I::B / 2)
                .copy_to_nonoverlapping(dest.0.children_ptr(pool), I::B / 2);

            // and then fill our AND dest's second half of pivots with I::MAX
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
}

impl NodeRef {
    #[inline]
    unsafe fn starts_ptr<I: RangeTreeInteger, V>(
        self,
        pool: &NodePool<I, marker::Leaf<V>>,
    ) -> NonNull<MaybeUninit<I::Raw>> {
        #[cfg(debug_assertions)]
        self.assert_valid(pool);

        let (_, starts_offset, _, _) = const { leaf_node_layout::<I, V>() };
        unsafe {
            let ptr = pool.ptr.byte_add(self.0 as usize);
            ptr.byte_add(starts_offset).cast::<MaybeUninit<I::Raw>>()
        }
    }

    #[inline]
    pub(crate) unsafe fn values_ptr<I: RangeTreeInteger, V>(
        self,
        pool: &NodePool<I, marker::Leaf<V>>,
    ) -> NonNull<MaybeUninit<V>> {
        #[cfg(debug_assertions)]
        self.assert_valid(pool);

        let (_, _, values_offset, _) = const { leaf_node_layout::<I, V>() };
        unsafe {
            let ptr = pool.ptr.byte_add(self.0 as usize);
            ptr.byte_add(values_offset).cast::<MaybeUninit<V>>()
        }
    }

    #[inline]
    unsafe fn next_leaf_ptr<I: RangeTreeInteger, V>(
        self,
        pool: &NodePool<I, marker::Leaf<V>>,
    ) -> NonNull<NodeRef> {
        #[cfg(debug_assertions)]
        self.assert_valid(pool);

        let (_, _, _, next_leaf_offset) = const { leaf_node_layout::<I, V>() };
        unsafe {
            let ptr = pool.ptr.byte_add(self.0 as usize);
            ptr.byte_add(next_leaf_offset).cast::<NodeRef>()
        }
    }

    #[inline]
    pub(crate) unsafe fn start<I: RangeTreeInteger, V>(
        self,
        pos: NodePos<I>,
        pool: &NodePool<I, marker::Leaf<V>>,
    ) -> MaybeUninit<I::Raw> {
        unsafe { self.starts_ptr(pool).add(pos.index()).read() }
    }

    #[inline]
    pub(crate) unsafe fn start_mut<I: RangeTreeInteger, V>(
        self,
        pos: NodePos<I>,
        pool: &mut NodePool<I, marker::Leaf<V>>,
    ) -> &mut MaybeUninit<I::Raw> {
        unsafe { self.starts_ptr(pool).add(pos.index()).as_mut() }
    }

    #[inline]
    pub(crate) unsafe fn insert_start<I: RangeTreeInteger, V>(
        self,
        start: I::Raw,
        pos: NodePos<I>,
        node_size: usize,
        pool: &mut NodePool<I, marker::Leaf<V>>,
    ) {
        debug_assert!(node_size <= I::B);
        debug_assert!(node_size > pos.index());
        unsafe {
            let ptr = self.starts_ptr(pool).add(pos.index());
            let count = node_size - pos.index() - 1;
            ptr.copy_to(ptr.add(1), count);
            ptr.write(MaybeUninit::new(start));
        }
    }

    // #[inline]
    // pub(crate) unsafe fn value<I: RangeTreeInteger, V>(
    //     self,
    //     pos: NodePos<I>,
    //     pool: &NodePool<I, marker::Leaf<V>>,
    // ) -> &MaybeUninit<V> {
    //     unsafe { self.values_ptr(pool).add(pos.index()).as_ref() }
    // }

    #[inline]
    pub(crate) unsafe fn value_mut<I: RangeTreeInteger, V>(
        self,
        pos: NodePos<I>,
        pool: &mut NodePool<I, marker::Leaf<V>>,
    ) -> &mut MaybeUninit<V> {
        unsafe { self.values_ptr(pool).add(pos.index()).as_mut() }
    }

    #[inline]
    pub(crate) unsafe fn insert_value<I: RangeTreeInteger, V>(
        self,
        value: V,
        pos: NodePos<I>,
        node_size: usize,
        pool: &mut NodePool<I, marker::Leaf<V>>,
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

    #[inline]
    pub(crate) unsafe fn next_leaf<I: RangeTreeInteger, V>(
        self,
        pool: &NodePool<I, marker::Leaf<V>>,
    ) -> Option<NodeRef> {
        let next_leaf = unsafe { self.next_leaf_ptr(pool).read() };

        (next_leaf.0 != !0).then_some(next_leaf)
    }

    #[inline]
    pub(crate) unsafe fn set_next_leaf<I: RangeTreeInteger, V>(
        self,
        next_leaf: Option<NodeRef>,
        pool: &NodePool<I, marker::Leaf<V>>,
    ) {
        unsafe {
            self.next_leaf_ptr(pool)
                .write(next_leaf.unwrap_or(NodeRef(!0)))
        }
    }

    #[inline]
    pub(crate) unsafe fn leaf_split_into<I: RangeTreeInteger, V>(
        self,
        dest: UninitNodeRef,
        pool: &mut NodePool<I, marker::Leaf<V>>,
    ) -> Self {
        unsafe {
            // copy the second half of our node into dest
            self.pivots_ptr(pool)
                .add(I::B / 2)
                .copy_to_nonoverlapping(dest.0.pivots_ptr(pool), I::B / 2);
            self.starts_ptr(pool)
                .add(I::B / 2)
                .copy_to_nonoverlapping(dest.0.starts_ptr(pool), I::B / 2);
            self.values_ptr(pool)
                .add(I::B / 2)
                .copy_to_nonoverlapping(dest.0.values_ptr(pool), I::B / 2);

            // and then fill our AND dest's second half of pivots with I::MAX
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
}

#[derive(Debug)]
pub(crate) struct UninitNodeRef(NodeRef);

impl UninitNodeRef {
    /// Initializes all pivots of the node with `I::MAX`.
    ///
    /// # Safety
    ///
    /// `self` must be allocated from `pool`.
    #[inline]
    pub(crate) unsafe fn init_pivots<I: RangeTreeInteger, Type>(
        self,
        pool: &mut NodePool<I, Type>,
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

pub(crate) mod marker {
    use core::any::type_name;
    use core::fmt::{Debug, Formatter};
    use core::marker::PhantomData;

    #[derive(Debug)]
    pub(crate) enum Internal {}
    pub(crate) struct Leaf<V>(PhantomData<V>);

    impl<V> Debug for Leaf<V> {
        fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
            f.debug_tuple("Leaf").field(&type_name::<V>()).finish()
        }
    }
}

pub(crate) const fn internal_node_layout<I: RangeTreeInteger>() -> (Layout, usize) {
    let layout = Layout::new::<I::Pivots>();

    let Ok(children) = Layout::array::<NodeRef>(I::B) else {
        panic!("Could not calculate node layout");
    };
    let Ok((layout, children_offset)) = layout.extend(children) else {
        panic!("Could not calculate node layout");
    };

    // Freed nodes are kept as a linked list of NodeRef, so ensure we can fit a
    // NodeRef in the node.
    let Ok(layout) = layout.align_to(4) else {
        panic!("Could not calculate node layout");
    };

    (layout.pad_to_align(), children_offset)
}

pub(crate) const fn leaf_node_layout<I: RangeTreeInteger, V>() -> (Layout, usize, usize, usize) {
    let layout = Layout::new::<I::Pivots>();

    let Ok(starts) = Layout::array::<I::Raw>(I::B) else {
        panic!("Could not calculate node layout");
    };
    let Ok((layout, starts_offset)) = layout.extend(starts) else {
        panic!("Could not calculate node layout");
    };

    let Ok(values) = Layout::array::<V>(I::B - 1) else {
        panic!("Could not calculate node layout");
    };
    let Ok((layout, values_offset)) = layout.extend(values) else {
        panic!("Could not calculate node layout");
    };

    let next_leaf = Layout::new::<NodeRef>();
    let Ok((layout, next_leaf_offset)) = layout.extend(next_leaf) else {
        panic!("Could not calculate node layout");
    };

    // Freed nodes are kept as a linked list of NodeRef, so ensure we can fit a
    // NodeRef in the node.
    let Ok(layout) = layout.align_to(4) else {
        panic!("Could not calculate node layout");
    };

    (
        layout.pad_to_align(),
        starts_offset,
        values_offset,
        next_leaf_offset,
    )
}

pub(crate) struct NodePool<I: RangeTreeInteger, Type> {
    /// Base of the allocation.
    ptr: NonNull<u8>,

    /// Size of the allocation.
    capacity: u32,

    /// Amount of the allocation currently in use. This is always a multiple of
    /// the node size.
    len: u32,

    /// Linked list of freed nodes, terminated by `!0`.
    free_list: u32,

    _type: PhantomData<(I, Type)>,
}

impl<I: RangeTreeInteger, Type> NodePool<I, Type> {
    pub(crate) const fn new() -> Self {
        Self {
            ptr: NonNull::dangling(),
            capacity: 0,
            len: 0,
            free_list: !0,
            _type: PhantomData,
        }
    }

    /// Frees all `NodeRef`s allocated from this pool.
    pub(crate) fn clear(&mut self) {
        self.len = 0;
        self.free_list = !0;
    }

    #[inline]
    fn grow(&mut self, node_layout: Layout, allocator: &impl Allocator) -> Result<(), AllocError> {
        if self.capacity == 0 {
            // Allocate space for 2 nodes for the initial allocation.
            let new_layout = Layout::from_size_align(node_layout.size() * 2, node_layout.align())
                .expect("exceeded RangeTree maximum allocation size");

            assert!(
                new_layout.size() <= MAX_POOL_SIZE,
                "exceeded RangeTree maximum allocation size"
            );

            self.ptr = allocator.allocate(new_layout)?.cast();
            self.capacity = new_layout.size() as u32;
        } else {
            let old_layout = unsafe {
                Layout::from_size_align_unchecked(self.capacity as usize, node_layout.align())
            };

            // This multiplication cannot overflow because the capacity in a
            // layout cannot exceed `isize::MAX`.
            let new_layout =
                Layout::from_size_align(self.capacity as usize * 2, node_layout.align())
                    .expect("exceeded BTree maximum allocation size");
            assert!(
                new_layout.size() <= MAX_POOL_SIZE,
                "exceeded BTree maximum allocation size"
            );
            self.ptr = unsafe { allocator.grow(self.ptr, old_layout, new_layout)?.cast() };
            self.capacity = new_layout.size() as u32;
        }

        Ok(())
    }

    #[inline]
    unsafe fn alloc_node_inner(
        &mut self,
        node_layout: Layout,
        allocator: &impl Allocator,
    ) -> Result<UninitNodeRef, AllocError> {
        // First try re-using a node from the free list.
        if self.free_list != !0 {
            // Freed nodes hold a single `NodeRef` with the next element in the
            // free list.
            let node = UninitNodeRef(NodeRef(self.free_list));
            self.free_list = unsafe { self.ptr.byte_add(self.free_list as usize).cast().read() };
            return Ok(node);
        }

        if self.capacity == self.len {
            self.grow(node_layout, allocator)?;
        }

        // grow() will have doubled the capacity or initialized it, which
        // guarantees at least enough space to allocate a single node.
        let node = UninitNodeRef(NodeRef(self.len));
        self.len += node_layout.size() as u32;
        debug_assert!(self.len <= self.capacity);
        Ok(node)
    }

    /// Frees the pool and its allocation. This invalidates all `NodeRef`s
    /// allocated from this pool.
    ///
    /// # Safety
    ///
    /// This pool must always be used with the same allocator.
    #[inline]
    unsafe fn clear_and_free_inner(&mut self, node_layout: Layout, allocator: &impl Allocator) {
        self.clear();
        let layout = unsafe {
            Layout::from_size_align_unchecked(self.capacity as usize, node_layout.align())
        };
        unsafe {
            allocator.deallocate(self.ptr, layout);
        }
        self.capacity = 0;
    }
}

impl<I: RangeTreeInteger> NodePool<I, marker::Internal> {
    /// Allocates a new uninitialized internal node from the pool.
    ///
    /// # Safety
    ///
    /// This pool must always be used with the same allocator.
    pub(crate) unsafe fn alloc_node(
        &mut self,
        allocator: &impl Allocator,
    ) -> Result<UninitNodeRef, AllocError> {
        let (node_layout, _) = const { internal_node_layout::<I>() };

        unsafe { self.alloc_node_inner(node_layout, allocator) }
    }

    /// Frees the pool and its allocation. This invalidates all `NodeRef`s
    /// allocated from this pool.
    ///
    /// # Safety
    ///
    /// This pool must always be used with the same allocator.
    #[inline]
    pub(crate) unsafe fn clear_and_free(&mut self, allocator: &impl Allocator) {
        let (node_layout, _) = const { internal_node_layout::<I>() };
        unsafe { self.clear_and_free_inner(node_layout, allocator) }
    }
}

impl<I: RangeTreeInteger, V> NodePool<I, marker::Leaf<V>> {
    /// Allocates a new uninitialized leaf node from the pool.
    ///
    /// # Safety
    ///
    /// This pool must always be used with the same allocator.
    pub(crate) unsafe fn alloc_node(
        &mut self,
        allocator: &impl Allocator,
    ) -> Result<UninitNodeRef, AllocError> {
        let (node_layout, _, _, _) = const { leaf_node_layout::<I, V>() };

        unsafe { self.alloc_node_inner(node_layout, allocator) }
    }

    /// Same as `clear` but then allocates the first node.
    pub(crate) fn clear_and_alloc_node(&mut self) -> UninitNodeRef {
        let (node_layout, _, _, _) = const { leaf_node_layout::<I, V>() };
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
    pub(crate) unsafe fn clear_and_free(&mut self, allocator: &impl Allocator) {
        let (node_layout, _, _, _) = const { leaf_node_layout::<I, V>() };
        unsafe { self.clear_and_free_inner(node_layout, allocator) }
    }
}

#[cfg(test)]
mod tests {
    use nonmax::NonMaxU64;

    use super::*;

    #[test]
    fn layout() {
        let (layout, children_offset) = const { internal_node_layout::<NonMaxU64>() };

        assert!(layout.align() >= align_of::<NodeRef>());
        assert!(
            layout.size()
                >= (size_of::<u64>() * NonMaxU64::B) + (size_of::<NodeRef>() * NonMaxU64::B)
        );
        assert_eq!(children_offset, size_of::<u64>() * NonMaxU64::B);

        let (layout, starts_offset, values_offset, next_leaf_offset) =
            const { leaf_node_layout::<NonMaxU64, usize>() };

        let size_pivots = size_of::<u64>() * NonMaxU64::B;
        let size_starts = size_of::<u64>() * NonMaxU64::B;
        let size_values = size_of::<usize>() * (NonMaxU64::B - 1);

        assert!(layout.align() >= align_of::<NodeRef>());
        assert!(
            layout.size() >= size_pivots + size_starts + size_values + (size_of::<NodeRef>()), // next leaf
            "leaf node layout too small! must be at least "
        );
        assert_eq!(starts_offset, size_pivots);
        assert_eq!(values_offset, size_pivots + size_starts);
        assert_eq!(next_leaf_offset, size_pivots + size_starts + size_values);
    }
}
