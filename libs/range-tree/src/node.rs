use core::alloc::Layout;
use core::marker::PhantomData;
use core::ptr::NonNull;
use crate::Idx;

/// A reference to a node inside a `NodePool`.
///
/// This is encoded as a `u32` offset within the pool to save space.
///
/// This doesn't have a lifetime, but is logically bound to the `NodePool` that
/// it was allocated from and is only valid for the lifetime of that pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NodeRef(u32);

// const fn node_layout<I: Idx, V>() -> (Layout, usize, usize, usize) {
//     const { assert!(I::B >= 4) };
//     const { assert!(I::B.is_multiple_of(2)) };
//
//     const fn max(a: usize, b: usize) -> usize {
//         if a > b { a } else { b }
//     }
//
//     let pivots = Layout::new::<I::Keys>();
//     let Ok(values) = Layout::array::<V>(I::B - 1) else {
//         panic!("Could not calculate node layout");
//     };
// }


pub(crate) struct NodePool<I: Idx, V> {
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