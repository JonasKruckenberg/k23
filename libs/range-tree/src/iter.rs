use core::alloc::Allocator;
use core::marker::PhantomData;
use core::ops::RangeBounds;

use crate::{idx::Idx, RangeTree};

pub struct Iter<'t, I: Idx, V, A: Allocator> {
    _m: PhantomData<&'t (I, V, A)>,
}

pub struct IterMut<'t, I: Idx, V, A: Allocator> {
    _m: PhantomData<&'t (I, V, A)>,
}

pub struct Ranges<'t, I: Idx, V, A: Allocator> {
    _m: PhantomData<&'t (I, V, A)>,
}

pub struct Values<'t, I: Idx, V, A: Allocator> {
    _m: PhantomData<&'t (I, V, A)>,
}

pub struct ValuesMut<'t, I: Idx, V, A: Allocator> {
    _m: PhantomData<&'t (I, V, A)>,
}

pub struct Range<'t, I: Idx, V, A: Allocator> {
    _m: PhantomData<&'t (I, V, A)>,
}

pub struct RangeMut<'t, I: Idx, V, A: Allocator> {
    _m: PhantomData<&'t (I, V, A)>,
}

impl<I: Idx, V, A: Allocator> RangeTree<I, V, A> {
    pub fn iter(&self) -> Iter<'_, I, V, A> {
        todo!()
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, I, V, A> {
        todo!()
    }

    pub fn ranges(&self) -> Ranges<'_, I, V, A> {
        todo!()
    }

    pub fn values(&self) -> Values<'_, I, V, A> {
        todo!()
    }

    pub fn values_mut(&mut self) -> ValuesMut<'_, I, V, A> {
        todo!()
    }

    pub fn range(&self, range: impl RangeBounds<I>) -> Range<'_, I, V, A> {
        todo!()
    }

    pub fn range_mut(&mut self, range: impl RangeBounds<I>) -> RangeMut<'_, I, V, A> {
        todo!()
    }
}
