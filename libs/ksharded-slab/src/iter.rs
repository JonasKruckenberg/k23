use core::{iter::FusedIterator, slice};

use crate::{cfg, page, shard};

/// An exclusive fused iterator over the items in a [`Slab`](crate::Slab).
#[must_use = "iterators are lazy and do nothing unless consumed"]
#[derive(Debug)]
pub struct UniqueIter<'a, T, C: cfg::Config> {
    pub(super) shards: shard::IterMut<'a, Option<T>, C>,
    pub(super) pages: slice::Iter<'a, page::Shared<Option<T>, C>>,
    pub(super) slots: Option<page::Iter<'a, T, C>>,
}

impl<'a, T, C: cfg::Config> Iterator for UniqueIter<'a, T, C> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        log::trace!("UniqueIter::next");
        loop {
            log::trace!("-> try next slot");
            if let Some(item) = self.slots.as_mut().and_then(|slots| slots.next()) {
                log::trace!("-> found an item!");
                return Some(item);
            }

            log::trace!("-> try next page");
            if let Some(page) = self.pages.next() {
                log::trace!("-> found another page");
                self.slots = page.iter();
                continue;
            }

            log::trace!("-> try next shard");
            if let Some(shard) = self.shards.next() {
                log::trace!("-> found another shard");
                self.pages = shard.iter();
            } else {
                log::trace!("-> all done!");
                return None;
            }
        }
    }
}

impl<T, C: cfg::Config> FusedIterator for UniqueIter<'_, T, C> {}
