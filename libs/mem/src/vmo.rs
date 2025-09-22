// Copyright 2025. Jonas Kruckenberg
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use alloc::sync::Arc;
use core::alloc::Layout;
use core::ops::{Bound, Range, RangeBounds};
use core::{fmt, ptr};

use anyhow::{anyhow, ensure};
use fallible_iterator::FallibleIterator;
use lock_api::RwLock;
use smallvec::SmallVec;

use crate::frame_alloc::FrameAllocator;
use crate::{AccessRules, FrameRef};

pub struct Vmo {
    name: &'static str,
    vmo: RawVmo,
}

#[derive(Debug)]
struct RawVmo {
    data: *const (),
    vtable: &'static RawVmoVTable,
}

#[derive(PartialEq, Copy, Clone, Debug)]
struct RawVmoVTable {
    clone: unsafe fn(*const ()) -> RawVmo,
    acquire: unsafe fn(
        *const (),
        index: usize,
        access_rules: AccessRules,
    ) -> crate::Result<Option<FrameRef>>,
    release: unsafe fn(*const (), index: usize) -> crate::Result<Option<FrameRef>>,
    clear: unsafe fn(*const (), index: usize) -> crate::Result<Option<FrameRef>>,
    len: unsafe fn(*const ()) -> usize,
    resize: unsafe fn(*const (), new_len: usize) -> crate::Result<()>,
    drop: unsafe fn(*const ()),
}

// ===== impl Vmo =====

impl Unpin for Vmo {}

// Safety: As part of the safety contract for RawVmoVTable, the caller promised RawVmo is Send
// therefore Vmo is Send too
unsafe impl Send for Vmo {}
// Safety: As part of the safety contract for RawVmoVTable, the caller promised RawVmo is Sync
// therefore Vmo is Sync too
unsafe impl Sync for Vmo {}

impl Clone for Vmo {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            vmo: unsafe { (self.vmo.vtable.clone)(self.vmo.data) },
            name: self.name,
        }
    }
}

impl Drop for Vmo {
    #[inline]
    fn drop(&mut self) {
        unsafe { (self.vmo.vtable.drop)(self.vmo.data) }
    }
}

impl fmt::Debug for Vmo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let vtable_ptr = self.vmo.vtable as *const RawVmoVTable;
        f.debug_struct("Vmo")
            .field("name", &self.name)
            .field("data", &self.vmo.data)
            .field("vtable", &vtable_ptr)
            .finish()
    }
}

impl Vmo {
    /// Creates a new `Vmo` from the provided `len`, `data` pointer and `vtable`.
    ///
    /// TODO
    ///
    /// The `data` pointer can be used to store arbitrary data as required by the vmo implementation.
    /// This could be e.g. a type-erased pointer to an `Arc` that holds private implementation-specific state.
    /// The value of this pointer will get passed to all functions that are part
    /// of the `vtable` as the first parameter.
    ///
    /// It is important to consider that the `data` pointer must point to a
    /// thread safe type such as an `Arc`.
    ///
    /// The `vtable` customizes the behavior of a `Cmo`. For each operation
    /// on the `Clock`, the associated function in the `vtable` will be called.
    ///
    /// # Safety
    ///
    /// The behavior of the returned `Vmo` is undefined if the contract defined
    /// in [`RawVmoVTable`]'s documentation is not upheld.
    #[inline]
    #[must_use]
    pub const unsafe fn new(data: *const (), vtable: &'static RawVmoVTable) -> Self {
        // Safety: ensured by caller
        unsafe { Self::from_raw(RawVmo { data, vtable }) }
    }

    /// Creates a new `Vmo` from a [`RawVmo`].
    ///
    /// # Safety
    ///
    /// The behavior of the returned `Vmo` is undefined if the contract defined
    /// in [`RawVmo`]'s and [`RawVmoVTable`]'s documentation is not upheld.
    #[inline]
    #[must_use]
    pub const unsafe fn from_raw(vmo: RawVmo) -> Self {
        Self {
            vmo,
            name: "<unnamed mystery VMO>",
        }
    }

    /// Add an arbitrary user-defined name to this `Vmo`.
    pub fn named(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Returns this `Vmo`'s name, if it was given one using the [`Vmo::named`]
    /// method.
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn len(&self) -> usize {
        unsafe { (self.vmo.vtable.len)(self.vmo.data) }
    }

    pub fn has_content_source(&self) -> bool {
        self.content_source().is_some()
    }

    pub fn content_source(&self) -> Option<()> {
        todo!()
    }

    /// Gets the `data` pointer used to create this `Vmo`.
    #[inline]
    #[must_use]
    pub fn data(&self) -> *const () {
        self.vmo.data
    }

    /// Gets the `vtable` pointer used to create this `Vmo`.
    #[inline]
    #[must_use]
    pub fn vtable(&self) -> &'static RawVmoVTable {
        self.vmo.vtable
    }

    // Release the frame at the given `index`. After this call succeeds, all accessed following the
    // given `access_rules` MUST NOT fault.
    // UNIT: frames
    pub fn acquire<R>(
        &self,
        range: R,
        access_rules: AccessRules,
    ) -> impl FallibleIterator<Item = FrameRef, Error = anyhow::Error>
    where
        R: RangeBounds<usize>,
    {
        let range = self.bound_check(range);

        let i = range
            .into_iter()
            .flat_map(|r| r)
            .filter_map(move |idx| unsafe {
                (self.vmo.vtable.acquire)(self.vmo.data, idx, access_rules).transpose()
            });

        fallible_iterator::convert(i)
    }

    // Release the frame at the given `index`. After this call succeeds, all accessed to the frame
    // MUST fault. Returns the base physical address of the release frame.
    // UNIT: frames
    pub fn release<R>(
        &self,
        range: R,
    ) -> impl FallibleIterator<Item = FrameRef, Error = anyhow::Error>
    where
        R: RangeBounds<usize>,
    {
        let range = self.bound_check(range);

        let i = range
            .into_iter()
            .flat_map(|r| r)
            .filter_map(|idx| unsafe { (self.vmo.vtable.release)(self.vmo.data, idx).transpose() });

        fallible_iterator::convert(i)
    }

    // Release the frame at the given `index`. After this call succeeds, all accessed to the frame
    // MUST fault. Returns the base physical address of the release frame.
    // UNIT: frames
    pub fn clear<R>(
        &self,
        range: R,
    ) -> impl FallibleIterator<Item = FrameRef, Error = anyhow::Error>
    where
        R: RangeBounds<usize>,
    {
        let range = self.bound_check(range);

        let i = range
            .into_iter()
            .flat_map(|r| r)
            .filter_map(|idx| unsafe { (self.vmo.vtable.clear)(self.vmo.data, idx).transpose() });

        fallible_iterator::convert(i)
    }

    // Grow the VMO to `new_size` (guaranteed to be larger than or equal to the current size).
    fn grow(&self, new_len: usize) -> crate::Result<()> {
        debug_assert!(new_len >= self.len());

        unsafe { (self.vmo.vtable.resize)(self.vmo.data, new_len)? };

        Ok(())
    }

    // Shrink the VMO to `new_size` (guaranteed to be smaller than or equal to the current size).
    // After this call succeeds, all accesses outside the new range MUST fault.
    // UNIT: frames
    pub fn shrink(
        &self,
        new_len: usize,
    ) -> impl FallibleIterator<Item = FrameRef, Error = anyhow::Error> {
        debug_assert!(new_len <= self.len());

        let old_len = self.len();

        todo!();
        fallible_iterator::empty()
        // unsafe {
        //     (self.vmo.vtable.resize)(self.vmo.data, new_len)?;
        // };
        //
        // let i = (new_len..old_len)
        //     .into_iter()
        //     .filter_map(|idx| unsafe { (self.vmo.vtable.release)(self.vmo.data, idx).transpose() });
        //
        // fallible_iterator::convert(i)
    }

    #[inline]
    fn bound_check<R>(&self, range: R) -> crate::Result<Range<usize>>
    where
        R: RangeBounds<usize>,
    {
        let start = match range.start_bound() {
            Bound::Included(b) => *b,
            Bound::Excluded(b) => *b + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(b) => *b + 1,
            Bound::Excluded(b) => *b,
            Bound::Unbounded => self.len(),
        };

        ensure!(end <= self.len());

        Ok(start..end)
    }
}

// ===== impl RawVmo =====

impl RawVmo {
    /// Creates a new `RawVmo` from the provided `data` pointer and `vtable`.
    ///
    /// The `data` pointer can be used to store arbitrary data as required by the VMO implementation.
    /// his could be e.g. a type-erased pointer to an `Arc` that holds private implementation-specific state.
    /// The value of this pointer will get passed to all functions that are part
    /// of the `vtable` as the first parameter.
    ///
    /// It is important to consider that the `data` pointer must point to a
    /// thread safe type such as an `Arc`.
    ///
    /// The `vtable` customizes the behavior of a `Vmo`. For each operation
    /// on the `Vmo`, the associated function in the `vtable` will be called.
    #[inline]
    #[must_use]
    pub const fn new(data: *const (), vtable: &'static RawVmoVTable) -> Self {
        Self { data, vtable }
    }
}

// ===== impl RawVmoVTable =====

impl RawVmoVTable {
    pub const fn new(
        clone: unsafe fn(*const ()) -> RawVmo,
        acquire: unsafe fn(
            *const (),
            index: usize,
            access_rules: AccessRules,
        ) -> crate::Result<Option<FrameRef>>,
        release: unsafe fn(*const (), index: usize) -> crate::Result<Option<FrameRef>>,
        clear: unsafe fn(*const (), index: usize) -> crate::Result<Option<FrameRef>>,
        len: unsafe fn(*const ()) -> usize,
        resize: unsafe fn(*const (), new_len: usize) -> crate::Result<()>,
        drop: unsafe fn(*const ()),
    ) -> Self {
        Self {
            clone,
            acquire,
            release,
            clear,
            len,
            resize,
            drop,
        }
    }
}

pub fn stub_vmo() -> Vmo {
    const WIRED_VMO_VTABLE: RawVmoVTable = RawVmoVTable::new(
        stub_clone,
        stub_acquire,
        stub_release,
        stub_clear,
        stub_len,
        stub_resize,
        stub_drop,
    );

    unsafe fn stub_clone(ptr: *const ()) -> RawVmo {
        debug_assert!(ptr.is_null());
        RawVmo::new(ptr, &WIRED_VMO_VTABLE)
    }

    unsafe fn stub_acquire(
        ptr: *const (),
        _index: usize,
        _access_rules: AccessRules,
    ) -> crate::Result<Option<FrameRef>> {
        debug_assert!(ptr.is_null());
        unreachable!()
    }
    unsafe fn stub_release(ptr: *const (), _index: usize) -> crate::Result<Option<FrameRef>> {
        debug_assert!(ptr.is_null());
        unreachable!()
    }
    unsafe fn stub_clear(ptr: *const (), _index: usize) -> crate::Result<Option<FrameRef>> {
        debug_assert!(ptr.is_null());
        unreachable!()
    }
    unsafe fn stub_len(ptr: *const ()) -> usize {
        debug_assert!(ptr.is_null());
        unreachable!()
    }
    unsafe fn stub_resize(ptr: *const (), _new_len: usize) -> crate::Result<()> {
        debug_assert!(ptr.is_null());
        unreachable!()
    }
    unsafe fn stub_drop(ptr: *const ()) {
        debug_assert!(ptr.is_null());
    }

    unsafe { Vmo::new(ptr::null(), &WIRED_VMO_VTABLE) }
}

pub struct PagedVmo<R: lock_api::RawRwLock> {
    list: RwLock<R, SmallVec<[Option<FrameRef>; 64]>>,
    frame_alloc: &'static dyn FrameAllocator,
}

impl<R: lock_api::RawRwLock> PagedVmo<R> {
    const RAW_VMO_VTABLE: RawVmoVTable = RawVmoVTable::new(
        Self::clone_vmo,
        Self::acquire,
        Self::release,
        Self::clear,
        Self::len,
        Self::resize,
        Self::drop_vmo,
    );

    pub fn new(frame_alloc: &'static dyn FrameAllocator) -> Self {
        Self {
            list: RwLock::new(SmallVec::new()),
            frame_alloc,
        }
    }

    #[inline(always)]
    pub fn into_vmo(self: Arc<Self>) -> Vmo {
        unsafe { Vmo::new(Arc::into_raw(self) as *const (), &Self::RAW_VMO_VTABLE) }
    }

    fn allocate_frame(&self) -> FrameRef {
        let layout =
            Layout::from_size_align(self.frame_alloc.page_size(), self.frame_alloc.page_size())
                .unwrap();

        let frames = self.frame_alloc.allocate(layout).unwrap();
        debug_assert_eq!(frames.len(), 1);

        unsafe { FrameRef::from_raw_parts(frames.cast(), self.frame_alloc.clone()) }
    }

    fn clone_the_zero_frame(&self) -> FrameRef {
        todo!()
    }

    #[inline(always)]
    unsafe fn clone_vmo(vmo: *const ()) -> RawVmo {
        unsafe {
            Arc::increment_strong_count(vmo.cast::<Self>());
        }
        RawVmo::new(vmo, &Self::RAW_VMO_VTABLE)
    }

    unsafe fn drop_vmo(ptr: *const ()) {
        drop(unsafe { Arc::from_raw(ptr.cast::<Self>()) });
    }

    unsafe fn acquire(
        ptr: *const (),
        index: usize,
        access_rules: AccessRules,
    ) -> crate::Result<Option<FrameRef>> {
        let me = unsafe { ptr.cast::<Self>().as_ref().unwrap() };

        todo!()
    }

    unsafe fn release(ptr: *const (), index: usize) -> crate::Result<Option<FrameRef>> {
        let me = unsafe { ptr.cast::<Self>().as_ref().unwrap() };
        let mut list = me.list.write();

        let slot = list
            .get_mut(index)
            .ok_or(anyhow!("index out of bounds"))?
            .take();

        Ok(slot)
    }

    unsafe fn clear(ptr: *const (), index: usize) -> crate::Result<Option<FrameRef>> {
        let me = unsafe { ptr.cast::<Self>().as_ref().unwrap() };
        let mut list = me.list.write();

        let prev_frame = list
            .get_mut(index)
            .ok_or(anyhow!("index out of bounds"))?
            .replace(me.clone_the_zero_frame());

        Ok(prev_frame)
    }

    unsafe fn len(ptr: *const ()) -> usize {
        let me = unsafe { ptr.cast::<Self>().as_ref().unwrap() };
        let list = me.list.read();

        list.len()
    }

    unsafe fn resize(ptr: *const (), new_len: usize) -> crate::Result<()> {
        let me = unsafe { ptr.cast::<Self>().as_ref().unwrap() };
        let mut list = me.list.write();

        list.resize(new_len, None);

        Ok(())
    }
}

struct VVmo<R: lock_api::RawRwLock> {
    list: RwLock<R, SmallVec<[Option<FrameRef>; 64]>>,
    frame_alloc: &'static dyn FrameAllocator,
    the_zero_frame: FrameRef,
}

struct Batch {
    freed: SmallVec<[FrameRef; 4]>,
    allocated: SmallVec<[FrameRef; 4]>,
}

impl<R: lock_api::RawRwLock> VVmo<R> {
    fn allocate_one(&self) -> FrameRef {
        let layout =
            Layout::from_size_align(self.frame_alloc.page_size(), self.frame_alloc.page_size())
                .unwrap();
        let frame = self.frame_alloc.allocate(layout).unwrap();
        debug_assert_eq!(frame.len(), 1);
        unsafe { FrameRef::from_raw_parts(frame.cast(), self.frame_alloc.clone()) }
    }
    
    pub fn acquire(&self, index: usize, access_rules: AccessRules, batch: &mut Batch) {
        let mut list = self.list.write();

        if let Some(old_frame) = list.get(index).unwrap() {
            assert!(!old_frame.is_unique());

            if access_rules.is_read_only() {
            }


            

        } else {
            let new_frame = self.allocate_one();
            list.insert(index, Some(new_frame));
            // TODO report new_frame for mapping
        }
    }

    pub fn release(&self, range: Range<usize>, batch: &mut Batch) {}
    pub fn clear(&self, range: Range<usize>, batch: &mut Batch) {}
}
