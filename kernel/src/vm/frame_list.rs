// Copyright 2025 Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use crate::arch;
use crate::vm::frame_alloc::Frame;
use alloc::boxed::Box;
use core::fmt::Formatter;
use core::iter::{FlatMap, Flatten, FusedIterator};
use core::mem::offset_of;
use core::pin::Pin;
use core::ptr::NonNull;
use core::{array, fmt};
use pin_project::pin_project;

const FRAME_LIST_NODE_FANOUT: usize = 16;

pub struct FrameList {
    pub nodes: wavltree::WAVLTree<FrameListNode>,
    size: usize,
}

#[pin_project]
#[derive(Debug)]
pub struct FrameListNode {
    links: wavltree::Links<FrameListNode>,
    offset: usize,
    frames: [Option<Frame>; FRAME_LIST_NODE_FANOUT],
}

pub struct Cursor<'a> {
    cursor: wavltree::Cursor<'a, FrameListNode>,
    index_in_node: usize,
    offset: usize,
}

pub struct CursorMut<'a> {
    cursor: wavltree::CursorMut<'a, FrameListNode>,
    index_in_node: usize,
    offset: usize,
}

// =============================================================================
// FrameList
// =============================================================================

impl fmt::Debug for FrameList {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("FrameList")
            .field("size", &self.size)
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

impl FrameList {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn get(&self, offset: usize) -> Option<&Frame> {
        let node_offset = offset_to_node_offset(offset);
        let node = self.nodes.find(&node_offset).get()?;

        let page = node.frames.get(offset_to_node_index(offset))?;
        page.as_ref()
    }

    pub fn get_mut(&mut self, offset: usize) -> Option<&mut Frame> {
        let node_offset = offset_to_node_offset(offset);
        let node = Pin::into_inner(self.nodes.find_mut(&node_offset).get_mut()?);

        let page = node.frames.get_mut(offset_to_node_index(offset))?;
        page.as_mut()
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

    pub fn insert(&mut self, offset: usize, new: Frame) -> &mut Frame {
        let node_offset = offset_to_node_offset(offset);
        let node = Pin::into_inner(self.nodes.find_mut(&node_offset).get_mut().unwrap());

        let frame = node.frames.get_mut(offset_to_node_index(offset)).unwrap();
        frame.insert(new)
    }

    pub fn first(&self) -> Option<&Frame> {
        let node = self.nodes.front().get()?;
        node.frames.iter().find(|f| f.is_some())?.as_ref()
    }

    pub fn last(&self) -> Option<&Frame> {
        let node = self.nodes.back().get()?;
        node.frames.iter().rfind(|f| f.is_some())?.as_ref()
    }

    pub fn cursor(&self, offset: usize) -> Cursor {
        let node_offset = offset_to_node_offset(offset);
        let cursor = self.nodes.find(&node_offset);

        Cursor {
            cursor,
            index_in_node: offset_to_node_index(offset),
            offset,
        }
    }

    pub fn cursor_mut(&mut self, offset: usize) -> CursorMut {
        let node_offset = offset_to_node_offset(offset);
        let cursor = self.nodes.find_mut(&node_offset);

        CursorMut {
            cursor,
            index_in_node: offset_to_node_index(offset),
            offset,
        }
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

    #[expect(tail_expr_drop_order, reason = "")]
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
                        links: wavltree::Links::default(),
                        offset,
                        frames: [const { None }; FRAME_LIST_NODE_FANOUT],
                    })
                });

            node.project().frames[offset_to_node_index(offset)] = Some(frame);
            offset += arch::PAGE_SIZE;
        }

        Self {
            nodes,
            size: offset,
        }
    }
}

// =============================================================================
// FrameListNode
// =============================================================================

// Safety: unsafe trait
unsafe impl wavltree::Linked for FrameListNode {
    type Handle = Pin<Box<FrameListNode>>;
    type Key = usize;

    fn into_ptr(handle: Self::Handle) -> NonNull<Self> {
        // Safety: wavltree treats the ptr as pinned
        unsafe { NonNull::from(Box::leak(Pin::into_inner_unchecked(handle))) }
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        // Safety: `NonNull` *must* be constructed from a pinned reference
        // which the tree implementation upholds.
        unsafe { Pin::new_unchecked(Box::from_raw(ptr.as_ptr())) }
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
    (offset) & 0usize.wrapping_sub(arch::PAGE_SIZE * FRAME_LIST_NODE_FANOUT)
}

fn offset_to_node_index(offset: usize) -> usize {
    (offset >> arch::PAGE_SHIFT) % FRAME_LIST_NODE_FANOUT
}

// =============================================================================
// Cursor
// =============================================================================

impl<'a> Cursor<'a> {
    /// Moves the cursor to the next [`Frame`] in the list
    pub fn move_next(&mut self) {
        self.offset += arch::PAGE_SIZE;

        // if there is a current node AND the node still has unseen frames in it
        // advance the offset
        if let Some(node) = self.cursor.get() {
            self.index_in_node += 1;
            if node.frames.len() > self.index_in_node {
                return;
            }
        }

        // otherwise advance the cursor and reset the offset
        self.cursor.move_next();
        self.index_in_node = 0;
    }

    /// Returns the offset of the [`Frame`] in this list, will always be a multiple
    /// of [`arch::PAGE_SIZE`].
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Returns a reference to the current [`Frame`] if any.
    pub fn get(&self) -> Option<&'a Frame> {
        let node = self.cursor.get()?;
        node.frames.get(self.index_in_node)?.as_ref()
    }
}

// =============================================================================
// CursorMut
// =============================================================================

impl<'a> CursorMut<'a> {
    /// Moves the cursor to the next [`Frame`] in the list
    pub fn move_next(&mut self) {
        self.offset += arch::PAGE_SIZE;

        // if there is a current node AND the node still has unseen frames in it
        // advance the index
        if let Some(node) = self.cursor.get() {
            self.index_in_node += 1;
            if node.frames.len() > self.index_in_node {
                return;
            }
        }

        // otherwise advance the cursor and reset the index
        self.cursor.move_next();
        self.index_in_node = 0;
    }

    /// Returns the offset of the [`Frame`] in this list, will always be a multiple
    /// of [`arch::PAGE_SIZE`].
    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn remove(&mut self) -> Option<Frame> {
        let node = Pin::into_inner(self.cursor.get_mut()?);
        let frame = node.frames.get_mut(self.index_in_node)?.take()?;

        // if the node has become empty remove it too
        if node.frames.iter().all(Option::is_none) {
            let _node = self.cursor.remove();
            self.index_in_node = 0;
        }

        Some(frame)
    }

    /// Returns a reference to the current [`Frame`] if any.
    pub fn get(&self) -> Option<&'a Frame> {
        let node = self.cursor.get()?;
        node.frames.get(self.index_in_node)?.as_ref()
    }

    /// Returns a mutable reference to the current [`Frame`] if any.
    pub fn get_mut(&mut self) -> Option<&mut Frame> {
        let node = Pin::into_inner(self.cursor.get_mut()?);
        node.frames.get_mut(self.index_in_node)?.as_mut()
    }
}
