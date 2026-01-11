use core::alloc::Allocator;
use core::marker::PhantomData;
use core::ops::Bound;

use crate::idx::Idx;
use crate::RangeTree;

pub struct Cursor<'t, I: Idx, V, A: Allocator> {
    _m: PhantomData<&'t (I, V, A)>,
}

pub struct CursorMut<'t, I: Idx, V, A: Allocator> {
    _m: PhantomData<&'t (I, V, A)>,
}

impl<I: Idx, V, A: Allocator> RangeTree<I, V, A> {
    pub fn cursor(&self) -> Cursor<'_, I, V, A> {
        todo!()
    }

    pub fn cursor_at(&self, bound: Bound<I>) -> Cursor<'_, I, V, A> {
        todo!()
    }

    pub fn cursor_mut(&mut self) -> CursorMut<'_, I, V, A> {
        todo!()
    }

    pub fn cursor_mut_at(&mut self, bound: Bound<I>) -> CursorMut<'_, I, V, A> {
        todo!()
    }
}
