use super::frame::Frame;
use alloc::boxed::Box;
use core::fmt::Formatter;
use core::iter::{FlatMap, Flatten, FusedIterator};
use core::mem::offset_of;
use core::pin::Pin;
use core::ptr::NonNull;
use core::{array, fmt};
use mmu::arch::PAGE_SIZE;
use pin_project::pin_project;

const FRAME_LIST_NODE_FANOUT: usize = 16;

pub struct FrameList {
    nodes: wavltree::WAVLTree<FrameListNode>,
    len: usize,
}

#[pin_project]
#[derive(Debug)]
struct FrameListNode {
    links: wavltree::Links<FrameListNode>,
    offset: usize,
    frames: [Option<Frame>; FRAME_LIST_NODE_FANOUT],
}

// =============================================================================
// FrameList
// =============================================================================

impl fmt::Debug for FrameList {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameList")
            .field_with("nodes", |f| {
                let mut f = f.debug_list();
                self.nodes.iter().for_each(|node| {
                    f.entry(node);
                });
                f.finish()
            })
            .finish()
    }
}

// impl Drop for FrameList {
//     fn drop(&mut self) {
//         self.clear();
//     }
// }

impl FrameList {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn get(&self, offset: usize) -> Option<&Frame> {
        let node_offset = offset_to_node_offset(offset);
        let node = self.nodes.find(&node_offset).get()?;

        let page = node.frames.get(offset_to_node_index(offset))?;
        page.as_ref()
    }

    pub fn take(&mut self, offset: usize) -> Option<Frame> {
        let node_offset = offset_to_node_offset(offset);
        let mut node = self.nodes.find_mut(&node_offset).get_mut()?;

        let page = node.frames.get_mut(offset_to_node_index(offset))?;
        page.take()
    }

    pub fn replace(&mut self, offset: usize, new: Frame) -> Option<Frame> {
        let node_offset = offset_to_node_offset(offset);
        let mut node = self.nodes.find_mut(&node_offset).get_mut()?;

        let page = node.frames.get_mut(offset_to_node_index(offset))?;
        page.replace(new)
    }

    pub fn first(&self) -> Option<&Frame> {
        let node = self.nodes.front().get()?;
        node.frames.iter().find(|f| f.is_some())?.as_ref()
    }

    pub fn last(&self) -> Option<&Frame> {
        let node = self.nodes.back().get()?;
        node.frames.iter().rfind(|f| f.is_some())?.as_ref()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Frame> {
        self.nodes
            .iter()
            .flat_map(|node| node.frames.iter().filter_map(|f| f.as_ref()))
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
    }

    /// Asserts the frame list is in a valid state.
    pub fn assert_valid(&self) {
        self.nodes.assert_valid();
        self.iter().for_each(|frame| frame.assert_valid());
    }
}

// =============================================================================
// FrameList IntoIterator
// =============================================================================

type FramesWithoutHoles = Flatten<array::IntoIter<Option<Frame>, FRAME_LIST_NODE_FANOUT>>;
type IntoIterInner = FlatMap<
    wavltree::IntoIter<FrameListNode>,
    FramesWithoutHoles,
    fn(Pin<Box<FrameListNode>>) -> FramesWithoutHoles,
>;

pub struct IntoIter(IntoIterInner);
impl Iterator for IntoIter {
    type Item = Frame;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}
impl DoubleEndedIterator for IntoIter {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.0.next_back()
    }
}
impl FusedIterator for IntoIter {}

impl IntoIterator for FrameList {
    type Item = Frame;
    type IntoIter = IntoIter;

    fn into_iter(mut self) -> Self::IntoIter {
        let inner: IntoIterInner = self
            .nodes
            .take()
            .into_iter()
            .flat_map(|node| Pin::into_inner(node).frames.into_iter().flatten());

        IntoIter(inner)
    }
}

impl FromIterator<Frame> for FrameList {
    fn from_iter<T: IntoIterator<Item = Frame>>(iter: T) -> Self {
        let mut nodes: wavltree::WAVLTree<FrameListNode> = wavltree::WAVLTree::new();

        let mut offset = 0;
        for frame in iter.into_iter() {
            let node = nodes
                .entry(&offset_to_node_offset(offset))
                .or_insert_with(|| {
                    Box::pin(FrameListNode {
                        links: Default::default(),
                        offset,
                        frames: [const { None }; FRAME_LIST_NODE_FANOUT],
                    })
                });

            node.project().frames[offset_to_node_index(offset)] = Some(frame);
            offset += PAGE_SIZE;
        }

        Self {
            nodes,
            len: offset / PAGE_SIZE,
        }
    }
}

// =============================================================================
// FrameListNode
// =============================================================================

unsafe impl wavltree::Linked for FrameListNode {
    type Handle = Pin<Box<FrameListNode>>;
    type Key = usize;

    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        unsafe { NonNull::from(Box::leak(Pin::into_inner_unchecked(handle))) }
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // Safety: `NonNull` *must* be constructed from a pinned reference
        // which the tree implementation upholds.
        Pin::new_unchecked(Box::from_raw(ptr.as_ptr()))
    }

    unsafe fn links(ptr: NonNull<Self>) -> NonNull<wavltree::Links<Self>> {
        ptr.map_addr(|addr| {
            let offset = offset_of!(Self, links);
            addr.checked_add(offset).unwrap()
        })
        .cast()
    }

    fn get_key(&self) -> &Self::Key {
        &self.offset
    }
}

fn offset_to_node_offset(offset: usize) -> usize {
    (offset) & 0usize.wrapping_sub(PAGE_SIZE * FRAME_LIST_NODE_FANOUT)
}

fn offset_to_node_index(offset: usize) -> usize {
    (offset >> mmu::arch::PAGE_SHIFT) % FRAME_LIST_NODE_FANOUT
}
